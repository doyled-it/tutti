// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure parsers for `gh --json` output. Kept separate from the shelling so they are
//! unit-testable against captured fixtures.

use serde::Deserialize;
use tutti_core::domain::{CiState, Issue, IssueId, IssueState, SelectFilter};
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
    // "OPEN" | "CLOSED". Absent from payloads captured before the field was requested,
    // which default to open.
    #[serde(default)]
    state: Option<String>,
    // GitHub's `issues` REST list returns pull requests too; a PR object carries a
    // `pull_request` block. Present only to detect and drop PRs in `parse_issue_list`.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct GhMilestone {
    title: String,
}

/// Every issue matching the filter, in forge order. The primitive: the adapter fetches
/// one page either way, so returning all matches lets a caller rank them (the milestone
/// floor) without paying one fetch per candidate ordering.
pub fn ready_issues(json: &str, filter: &SelectFilter) -> Vec<Issue> {
    let issues: Vec<GhIssue> = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    issues
        .into_iter()
        .map(to_issue)
        .filter(|i| {
            i.has_label(&filter.require_label)
                && !filter.skip_labels.iter().any(|s| i.has_label(s))
                && filter
                    .milestone
                    .as_ref()
                    .is_none_or(|m| i.milestone.as_ref() == Some(m))
        })
        .collect()
}

/// The first issue matching the filter. Derived from `ready_issues` so the two cannot drift.
pub fn first_ready_issue(json: &str, filter: &SelectFilter) -> Option<Issue> {
    ready_issues(json, filter).into_iter().next()
}

fn to_issue(g: GhIssue) -> Issue {
    Issue {
        id: IssueId(g.number),
        title: g.title,
        body: g.body,
        labels: g.labels.into_iter().map(|l| l.name).collect(),
        milestone: g.milestone.map(|m| m.title),
        state: issue_state(g.state.as_deref()),
    }
}

/// Map a forge state string to `IssueState`. Anything unrecognised (or absent) is treated
/// as open: mislabelling a closed issue as open is the safer error here, since `classify`
/// sends closed issues to Done where triage refuses to touch them.
fn issue_state(raw: Option<&str>) -> IssueState {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("closed") => IssueState::Closed,
        _ => IssueState::Open,
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
        due: normalize_due(m.due_on),
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

/// Parse `gh label list --json name,color` output (an array) into (name, color) pairs.
/// `gh` returns color as a hex string without a leading '#'; callers normalize.
pub fn parse_labels(json: &str) -> Vec<(String, String)> {
    #[derive(Deserialize)]
    struct L {
        name: String,
        color: String,
    }
    let raw: Vec<L> = serde_json::from_str(json).unwrap_or_default();
    raw.into_iter().map(|l| (l.name, l.color)).collect()
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

/// Normalise a forge due date to a plain ISO `YYYY-MM-DD`.
///
/// GitLab returns `due_date` already in that form, but GitHub and Gitea return `due_on` as
/// RFC 3339 (`2026-08-01T07:00:00Z`, and Gitea with a real offset like `+02:00`).
/// `milestone_floor_order` compares these as strings, which survives homogeneous UTC by luck
/// and breaks the moment offsets differ: `2026-08-01T00:00:00+02:00` is chronologically
/// BEFORE `2026-07-31T23:00:00Z` but sorts after it. Truncating at the `T` here keeps the
/// comparison honest, and makes `Milestone.due` actually match what its doc comment claims.
fn normalize_due(raw: Option<String>) -> Option<String> {
    raw.map(|d| d.split('T').next().unwrap_or(&d).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> SelectFilter {
        SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
            milestone: None,
            milestone_floor: false,
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
        // The captured fixture carries GitHub's real `due_on`, "2026-07-31T00:00:00Z".
        // Normalised on the way in, so `milestone_floor_order`'s string compare is actually
        // chronological rather than accidentally so.
        assert_eq!(open.due.as_deref(), Some("2026-07-31"));
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
    fn labels_parse_name_and_color() {
        let json = r#"[{"name":"status:ready","color":"00ff00"},{"name":"bug","color":"d73a4a"}]"#;
        let labels = parse_labels(json);
        assert_eq!(labels.len(), 2);
        assert!(labels
            .iter()
            .any(|(n, c)| n == "status:ready" && c == "00ff00"));
        assert!(labels.iter().any(|(n, c)| n == "bug" && c == "d73a4a"));
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

    #[test]
    fn issue_state_is_parsed_and_defaults_to_open() {
        // gh reports "OPEN"/"CLOSED" in caps. A payload captured before the field was
        // requested has no `state` at all and must decode as open, not silently closed.
        let json = r#"[
          {"number":1,"title":"a","body":"","labels":[],"milestone":null,"state":"CLOSED"},
          {"number":2,"title":"b","body":"","labels":[],"milestone":null,"state":"OPEN"},
          {"number":3,"title":"c","body":"","labels":[],"milestone":null}
        ]"#;
        let got = parse_issue_list(json);
        assert_eq!(got[0].state, IssueState::Closed);
        assert_eq!(got[1].state, IssueState::Open);
        assert_eq!(got[2].state, IssueState::Open, "absent means open");
    }

    #[test]
    fn milestone_due_is_normalized_to_a_plain_iso_date() {
        // GitHub returns `due_on` as RFC 3339. `milestone_floor_order` compares these as
        // strings, so leaving the time on makes the ordering accidental rather than correct.
        let json = r#"[{"number":1,"title":"v0.1","state":"open","due_on":"2026-08-01T07:00:00Z"},
                       {"number":2,"title":"v0.2","state":"open","due_on":null}]"#;
        let got = parse_milestones(json);
        assert_eq!(got[0].due.as_deref(), Some("2026-08-01"));
        assert_eq!(got[1].due, None);
    }
}
