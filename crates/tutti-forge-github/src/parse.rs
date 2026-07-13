// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure parsers for `gh --json` output. Kept separate from the shelling so they are
//! unit-testable against captured fixtures.

use serde::Deserialize;
use tutti_core::domain::{CiState, Issue, IssueId, SelectFilter};
use tutti_core::tracking::{Milestone, MilestoneId, Progress, TrackState};

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}
#[derive(Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default, rename = "milestone")]
    milestone: Option<GhMilestone>,
    // GitHub's `issues` REST list returns pull requests too; a PR object carries a
    // `pull_request` block. Present only to detect and drop PRs in `parse_issue_list`.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct GhMilestone {
    title: String,
}

/// Parse `gh issue list --json number,title,body,labels,milestone` output and return
/// the first issue that has `require_label` and none of `skip_labels`.
pub fn first_ready_issue(json: &str, filter: &SelectFilter) -> Option<Issue> {
    let issues: Vec<GhIssue> = serde_json::from_str(json).ok()?;
    issues.into_iter().map(to_issue).find(|i| {
        i.has_label(&filter.require_label)
            && !filter.skip_labels.iter().any(|s| i.has_label(s))
            && filter
                .milestone
                .as_ref()
                .is_none_or(|m| i.milestone.as_ref() == Some(m))
    })
}

fn to_issue(g: GhIssue) -> Issue {
    Issue {
        id: IssueId(g.number),
        title: g.title,
        body: g.body,
        labels: g.labels.into_iter().map(|l| l.name).collect(),
        milestone: g.milestone.map(|m| m.title),
    }
}

/// A `gh api .../milestones` object. Only the fields the engine needs are read; `gh`
/// returns many more, which serde ignores.
#[derive(Deserialize)]
struct GhMilestoneObj {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    due_on: Option<String>,
    #[serde(default)]
    open_issues: u32,
    #[serde(default)]
    closed_issues: u32,
}

fn milestone_from(m: GhMilestoneObj) -> Milestone {
    Milestone {
        id: MilestoneId(m.number),
        title: m.title,
        // The milestone's `Progress` is derived from GitHub's own counters:
        // total = open + closed, done = closed. An unrecognized state falls back to Open.
        state: match m.state.as_str() {
            "closed" => TrackState::Closed,
            _ => TrackState::Open,
        },
        due: m.due_on,
        progress: Progress {
            total: m.open_issues + m.closed_issues,
            done: m.closed_issues,
        },
    }
}

/// Parse `gh api repos/<repo>/milestones` output (an array) into `Milestone`s.
pub fn parse_milestones(json: &str) -> Vec<Milestone> {
    let raw: Vec<GhMilestoneObj> = serde_json::from_str(json).unwrap_or_default();
    raw.into_iter().map(milestone_from).collect()
}

/// Parse a single milestone object, as returned by a create (POST) response.
pub fn parse_milestone(json: &str) -> Option<Milestone> {
    let raw: GhMilestoneObj = serde_json::from_str(json).ok()?;
    Some(milestone_from(raw))
}

/// Parse a `gh api repos/<repo>/issues?...` array into `Issue`s. The `issues` REST
/// endpoint also returns pull requests (a PR carries a `pull_request` block), so those
/// are dropped: a PR filed under a milestone is not a child that must be `status:done`.
pub fn parse_issue_list(json: &str) -> Vec<Issue> {
    let issues: Vec<GhIssue> = serde_json::from_str(json).unwrap_or_default();
    issues
        .into_iter()
        .filter(|g| g.pull_request.is_none())
        .map(to_issue)
        .collect()
}

/// Parse `gh api repos/<repo>/issues/<n>/sub_issues` (an array of issue objects) into
/// the child issue ids.
pub fn parse_sub_issues(json: &str) -> Vec<IssueId> {
    #[derive(Deserialize)]
    struct Child {
        number: u64,
    }
    let children: Vec<Child> = serde_json::from_str(json).unwrap_or_default();
    children.into_iter().map(|c| IssueId(c.number)).collect()
}

/// Parse an issue's `sub_issues_summary` block into a `Progress` (total children and how
/// many are completed). Accepts either a full issue object carrying the summary or the
/// bare summary object.
pub fn parse_summary(json: &str) -> Progress {
    #[derive(Deserialize)]
    struct Summary {
        #[serde(default)]
        total: u32,
        #[serde(default)]
        completed: u32,
    }
    #[derive(Deserialize)]
    struct Wrapper {
        sub_issues_summary: Summary,
    }
    // Prefer the wrapped form (an issue object); fall back to a bare summary object.
    if let Ok(w) = serde_json::from_str::<Wrapper>(json) {
        return Progress {
            total: w.sub_issues_summary.total,
            done: w.sub_issues_summary.completed,
        };
    }
    match serde_json::from_str::<Summary>(json) {
        Ok(s) => Progress {
            total: s.total,
            done: s.completed,
        },
        Err(_) => Progress::default(),
    }
}

/// Parse a `gh api --method POST repos/<repo>/issues` create response into an `Issue`.
/// The response is a single issue object (`number,title,body,labels,milestone`).
pub fn parse_created_issue(json: &str) -> Option<Issue> {
    let g: GhIssue = serde_json::from_str(json).ok()?;
    Some(to_issue(g))
}

/// Parse the PR number from `gh pr create` output. gh prints the PR URL, sometimes
/// followed by an informational line, so take the last non-empty line and then its last
/// `/`-segment. Returns None if no line yields a numeric trailing segment.
pub fn parse_pr_number(out: &str) -> Option<u64> {
    let line = out.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    line.rsplit('/').next()?.trim().parse::<u64>().ok()
}

/// Map `gh pr checks --json state` output to a single `CiState`: Fail if any failed,
/// Pending if any pending/queued, else Pass. Unknown states are treated as Pending.
pub fn overall_ci_state(json: &str) -> CiState {
    #[derive(Deserialize)]
    struct Check {
        #[serde(default)]
        state: String,
    }
    let checks: Vec<Check> = match serde_json::from_str(json) {
        Ok(c) => c,
        Err(_) => return CiState::Pending,
    };
    if checks.is_empty() {
        return CiState::Pending;
    }
    let mut any_pending = false;
    for c in &checks {
        match c.state.to_uppercase().as_str() {
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" => {
                return CiState::Fail
            }
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            _ => any_pending = true, // PENDING, QUEUED, IN_PROGRESS, unknown
        }
    }
    if any_pending {
        CiState::Pending
    } else {
        CiState::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> SelectFilter {
        SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
            milestone: None,
        }
    }

    #[test]
    fn picks_first_ready_skipping_needs_human() {
        let json = r#"[
          {"number":1,"title":"a","body":"","labels":[{"name":"status:ready"},{"name":"status:needs-human"}],"milestone":null},
          {"number":2,"title":"b","body":"x","labels":[{"name":"status:ready"}],"milestone":{"title":"Phase 1"}}
        ]"#;
        let issue = first_ready_issue(json, &filter()).unwrap();
        assert_eq!(issue.id.0, 2);
        assert_eq!(issue.milestone.as_deref(), Some("Phase 1"));
    }

    #[test]
    fn ci_fail_wins() {
        assert_eq!(
            overall_ci_state(r#"[{"state":"SUCCESS"},{"state":"FAILURE"}]"#),
            CiState::Fail
        );
    }
    #[test]
    fn ci_pending_when_any_pending() {
        assert_eq!(
            overall_ci_state(r#"[{"state":"SUCCESS"},{"state":"PENDING"}]"#),
            CiState::Pending
        );
    }
    #[test]
    fn ci_pass_when_all_success() {
        assert_eq!(
            overall_ci_state(r#"[{"state":"SUCCESS"},{"state":"SKIPPED"}]"#),
            CiState::Pass
        );
    }
    #[test]
    fn ci_empty_is_pending() {
        assert_eq!(overall_ci_state("[]"), CiState::Pending);
    }

    #[test]
    fn pr_number_simple_url() {
        assert_eq!(
            parse_pr_number("https://github.com/o/r/pull/123"),
            Some(123)
        );
    }

    #[test]
    fn pr_number_trailing_newline() {
        assert_eq!(
            parse_pr_number("https://github.com/o/r/pull/123\n"),
            Some(123)
        );
    }

    #[test]
    fn pr_number_multiline_with_info_line() {
        // gh sometimes prints an informational line after the URL; the URL is not last.
        let out = "https://github.com/o/r/pull/456\nWarning: some notice\n";
        // The last non-empty line is not a URL, so this must fail to parse rather than
        // silently returning a wrong number.
        assert_eq!(parse_pr_number(out), None);
    }

    #[test]
    fn pr_number_url_is_last_nonempty_line() {
        let out = "Creating pull request for feat/x into main\nhttps://github.com/o/r/pull/789\n\n";
        assert_eq!(parse_pr_number(out), Some(789));
    }

    #[test]
    fn pr_number_garbage_is_none() {
        assert_eq!(parse_pr_number("not a url"), None);
    }

    // --- tracking parsers, against fixtures captured from real `gh api` output ---

    #[test]
    fn milestones_map_state_and_progress() {
        let json = include_str!("../tests/fixtures/milestones.json");
        let ms = parse_milestones(json);
        assert_eq!(ms.len(), 2);

        // The closed milestone (no children) maps state and zeroed progress.
        let closed = ms.iter().find(|m| m.id == MilestoneId(2)).unwrap();
        assert_eq!(closed.state, TrackState::Closed);
        assert_eq!(closed.title, "fixture-ms-closed");
        assert_eq!(closed.due, None);
        assert_eq!(closed.progress, Progress { total: 0, done: 0 });

        // The open milestone: total = open_issues + closed_issues, done = closed_issues.
        let open = ms.iter().find(|m| m.id == MilestoneId(1)).unwrap();
        assert_eq!(open.state, TrackState::Open);
        assert_eq!(open.due.as_deref(), Some("2026-07-31T00:00:00Z"));
        assert_eq!(open.progress, Progress { total: 3, done: 1 });
    }

    #[test]
    fn sub_issues_yield_child_ids() {
        let json = include_str!("../tests/fixtures/sub_issues.json");
        let ids = parse_sub_issues(json);
        assert_eq!(ids, vec![IssueId(6), IssueId(7)]);
    }

    #[test]
    fn summary_rolls_up_completed() {
        let json = include_str!("../tests/fixtures/issue_with_summary.json");
        let p = parse_summary(json);
        assert_eq!(p, Progress { total: 2, done: 1 });
    }

    #[test]
    fn summary_accepts_bare_object() {
        // The parser also accepts a bare `sub_issues_summary` payload.
        let p = parse_summary(r#"{"total":5,"completed":2,"percent_completed":40}"#);
        assert_eq!(p, Progress { total: 5, done: 2 });
    }

    #[test]
    fn issue_list_excludes_pull_requests() {
        // GitHub's issues?milestone= list returns PRs too; only the real issue is a child.
        let json = include_str!("../tests/fixtures/milestone_children.json");
        let issues = parse_issue_list(json);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, IssueId(11));
        assert!(issues[0].has_label("status:done"));
    }

    #[test]
    fn created_issue_carries_labels_and_milestone() {
        let json = include_str!("../tests/fixtures/created_issue.json");
        let issue = parse_created_issue(json).unwrap();
        assert_eq!(issue.id, IssueId(5));
        assert_eq!(issue.title, "fixture-epic");
        assert_eq!(issue.body, "parent");
        assert_eq!(issue.labels, vec!["status:ready".to_string()]);
        assert_eq!(issue.milestone.as_deref(), Some("fixture-ms"));
    }
}
