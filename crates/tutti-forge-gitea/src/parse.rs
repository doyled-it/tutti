// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure parsers for Gitea REST JSON (via `tea api`). Kept separate from the shelling
//! so they are unit-testable against captured fixtures.

use serde::Deserialize;
use tutti_core::domain::{Issue, IssueId, SelectFilter};
use tutti_core::tracking::{Milestone, MilestoneId, Progress, TrackState};

#[derive(Deserialize)]
struct GtLabel {
    name: String,
}
#[derive(Deserialize)]
struct GtMilestoneRef {
    title: String,
}
#[derive(Deserialize)]
struct GtIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<GtLabel>,
    #[serde(default)]
    milestone: Option<GtMilestoneRef>,
    // Gitea's issues list returns pull requests too; a PR carries a non-null
    // `pull_request` block. Present only to detect and drop PRs.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

fn to_issue(g: GtIssue) -> Issue {
    Issue {
        id: IssueId(g.number),
        title: g.title,
        body: g.body,
        labels: g.labels.into_iter().map(|l| l.name).collect(),
        milestone: g.milestone.map(|m| m.title),
    }
}

/// Parse a `GET issues` array and return the first issue matching the filter
/// (has `require_label`, none of `skip_labels`, and the optional milestone scope).
pub fn first_ready_issue(json: &str, filter: &SelectFilter) -> Option<Issue> {
    let issues: Vec<GtIssue> = serde_json::from_str(json).ok()?;
    issues.into_iter().map(to_issue).find(|i| {
        i.has_label(&filter.require_label)
            && !filter.skip_labels.iter().any(|s| i.has_label(s))
            && filter
                .milestone
                .as_ref()
                .is_none_or(|m| i.milestone.as_ref() == Some(m))
    })
}

/// Parse a `GET issues` array into `Issue`s, dropping pull requests.
pub fn parse_issue_list(json: &str) -> Vec<Issue> {
    let issues: Vec<GtIssue> = serde_json::from_str(json).unwrap_or_default();
    issues
        .into_iter()
        .filter(|g| g.pull_request.is_none())
        .map(to_issue)
        .collect()
}

/// Parse a single issue object (a create/get response) into an `Issue`.
pub fn parse_created_issue(json: &str) -> Option<Issue> {
    let g: GtIssue = serde_json::from_str(json).ok()?;
    Some(to_issue(g))
}

#[derive(Deserialize)]
struct GtMilestone {
    id: u64,
    title: String,
    state: String,
    #[serde(default)]
    due_on: Option<String>,
    #[serde(default)]
    open_issues: u32,
    #[serde(default)]
    closed_issues: u32,
}

fn milestone_from(m: GtMilestone) -> Milestone {
    Milestone {
        id: MilestoneId(m.id),
        title: m.title,
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

/// Parse a `GET milestones` array into `Milestone`s.
pub fn parse_milestones(json: &str) -> Vec<Milestone> {
    let raw: Vec<GtMilestone> = serde_json::from_str(json).unwrap_or_default();
    raw.into_iter().map(milestone_from).collect()
}

/// Parse a single milestone object (a create/PATCH response) into a `Milestone`.
pub fn parse_milestone(json: &str) -> Option<Milestone> {
    let raw: GtMilestone = serde_json::from_str(json).ok()?;
    Some(milestone_from(raw))
}

/// Parse a `GET labels` array into (name, id) pairs, for resolving label names to
/// the numeric ids Gitea's issue-label endpoints require.
pub fn parse_label_ids(json: &str) -> Vec<(String, i64)> {
    #[derive(Deserialize)]
    struct L {
        id: i64,
        name: String,
    }
    let labels: Vec<L> = serde_json::from_str(json).unwrap_or_default();
    labels.into_iter().map(|l| (l.name, l.id)).collect()
}

/// Parse the `number` from a `POST pulls` create response.
pub fn parse_created_pr_number(json: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Pr {
        number: u64,
    }
    serde_json::from_str::<Pr>(json).ok().map(|p| p.number)
}

/// Parse a PR's head commit SHA from a `GET pulls/{index}` response. Used to query the
/// combined commit status by SHA rather than by a slashed branch ref.
pub fn parse_pr_head_sha(json: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        sha: String,
    }
    #[derive(Deserialize)]
    struct Pr {
        #[serde(default)]
        head: Option<Head>,
    }
    let pr: Pr = serde_json::from_str(json).ok()?;
    pr.head.map(|h| h.sha).filter(|s| !s.is_empty())
}

/// Map a Gitea combined commit status (`GET commits/{ref}/status`) to a `CiState`.
/// Gitea's combined `state` is one of success|pending|failure|error|warning.
pub fn combined_ci_state(json: &str) -> tutti_core::domain::CiState {
    use tutti_core::domain::CiState;
    #[derive(Deserialize)]
    struct Combined {
        #[serde(default)]
        state: String,
        // When there are no statuses at all, treat as pending (CI not reported yet).
        #[serde(default)]
        total_count: u64,
    }
    let c: Combined = match serde_json::from_str(json) {
        Ok(c) => c,
        Err(_) => return CiState::Pending,
    };
    if c.total_count == 0 {
        return CiState::Pending;
    }
    match c.state.to_lowercase().as_str() {
        "success" => CiState::Pass,
        "failure" | "error" => CiState::Fail,
        _ => CiState::Pending, // pending, warning, unknown
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
    fn milestones_map_id_state_and_progress() {
        let json = include_str!("../tests/fixtures/milestones.json");
        let ms = parse_milestones(json);
        assert_eq!(ms.len(), 1);
        // The sandbox's real "Phase 1" milestone: id 135433, open, one open issue.
        let phase1 = ms.iter().find(|m| m.title == "Phase 1").unwrap();
        assert_eq!(phase1.id, MilestoneId(135433));
        assert_eq!(phase1.state, TrackState::Open);
        assert_eq!(phase1.due, None);
        assert_eq!(phase1.progress, Progress { total: 1, done: 0 });
    }

    #[test]
    fn labels_resolve_names_to_ids() {
        let json = include_str!("../tests/fixtures/labels.json");
        let ids = parse_label_ids(json);
        // The sandbox has the three status labels; each name maps to a positive id.
        assert_eq!(ids.len(), 3);
        assert!(ids
            .iter()
            .any(|(n, id)| n == "status:ready" && *id == 1993201));
        assert!(ids
            .iter()
            .any(|(n, id)| n == "status:in-progress" && *id == 1993204));
        assert!(ids
            .iter()
            .any(|(n, id)| n == "status:done" && *id == 1993207));
    }

    #[test]
    fn issue_list_excludes_pull_requests() {
        let json = include_str!("../tests/fixtures/milestone_children.json");
        let issues = parse_issue_list(json);
        // The synthetic PR element is dropped; only the real captured issue remains.
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, IssueId(1));
        assert_eq!(issues[0].title, "first sandbox issue");
        assert!(issues[0].has_label("status:in-progress"));
        assert_eq!(issues[0].milestone.as_deref(), Some("Phase 1"));
    }

    #[test]
    fn created_issue_carries_fields() {
        let json = include_str!("../tests/fixtures/created_issue.json");
        let issue = parse_created_issue(json).unwrap();
        assert_eq!(issue.id, IssueId(2));
        assert_eq!(issue.title, "fixture-issue");
        assert_eq!(issue.body, "fixture body");
        assert!(issue.labels.is_empty());
        assert_eq!(issue.milestone, None);
    }

    #[test]
    fn pr_head_sha_parses_and_rejects_empty() {
        assert_eq!(
            parse_pr_head_sha(r#"{"number":7,"head":{"ref":"feat/issue-7","sha":"abc123"}}"#),
            Some("abc123".to_string())
        );
        // An empty or missing sha yields None (treated as not-yet-reported by ci_status).
        assert_eq!(parse_pr_head_sha(r#"{"number":7,"head":{"sha":""}}"#), None);
        assert_eq!(parse_pr_head_sha(r#"{"number":7}"#), None);
    }

    #[test]
    fn ci_states_map() {
        assert_eq!(
            combined_ci_state(r#"{"state":"success","total_count":2}"#),
            tutti_core::domain::CiState::Pass
        );
        assert_eq!(
            combined_ci_state(r#"{"state":"failure","total_count":2}"#),
            tutti_core::domain::CiState::Fail
        );
        assert_eq!(
            combined_ci_state(r#"{"state":"pending","total_count":0}"#),
            tutti_core::domain::CiState::Pending
        );
    }
}
