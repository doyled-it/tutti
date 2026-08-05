// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure parsers for Gitea REST JSON (via `tea api`). Kept separate from the shelling
//! so they are unit-testable against captured fixtures.

use serde::Deserialize;
use tutti_core::domain::{Issue, IssueId, IssueState, SelectFilter};
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
    // "open" | "closed".
    #[serde(default)]
    state: Option<String>,
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

/// The number of elements in a JSON array page, before any filtering. Pagination must test
/// this rather than the parsed count: `parse_issue_list` drops pull requests, so a full page
/// can parse short, and treating that as the last page would silently truncate the backlog.
pub fn page_len(json: &str) -> usize {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .map(|v| v.len())
        .unwrap_or(0)
}

/// Every issue matching the filter, in forge order. The primitive: the adapter fetches
/// one page either way, so returning all matches lets a caller rank them (the milestone
/// floor) without paying one fetch per candidate ordering.
pub fn ready_issues(json: &str, filter: &SelectFilter) -> Vec<Issue> {
    let issues: Vec<GtIssue> = match serde_json::from_str(json) {
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
        due: normalize_due(m.due_on),
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

/// Parse a `GET labels` array into (name, color) pairs. Gitea returns color as a hex
/// string without a leading '#'; callers normalize.
pub fn parse_labels(json: &str) -> Vec<(String, String)> {
    #[derive(Deserialize)]
    struct L {
        name: String,
        color: String,
    }
    let labels: Vec<L> = serde_json::from_str(json).unwrap_or_default();
    labels.into_iter().map(|l| (l.name, l.color)).collect()
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
    fn labels_parse_name_and_color() {
        let json = include_str!("../tests/fixtures/labels.json");
        let labels = parse_labels(json);
        assert_eq!(labels.len(), 3);
        assert!(labels
            .iter()
            .any(|(n, c)| n == "status:ready" && c == "00ff00"));
        assert!(labels
            .iter()
            .any(|(n, c)| n == "status:in-progress" && c == "ffff00"));
        assert!(labels
            .iter()
            .any(|(n, c)| n == "status:done" && c == "0000ff"));
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

    #[test]
    fn issue_state_is_parsed_and_defaults_to_open() {
        let json = r#"[
          {"number":1,"title":"a","state":"closed"},
          {"number":2,"title":"b","state":"open"},
          {"number":3,"title":"c"}
        ]"#;
        let got = parse_issue_list(json);
        assert_eq!(got[0].state, IssueState::Closed);
        assert_eq!(got[1].state, IssueState::Open);
        assert_eq!(got[2].state, IssueState::Open, "absent means open");
    }

    #[test]
    fn page_len_counts_raw_elements_including_pull_requests() {
        // Pagination tests this, not the parsed length: a full page of mostly PRs parses
        // short, and treating that as the last page would truncate the backlog.
        let json = r#"[
          {"number":1,"title":"a"},
          {"number":2,"title":"pr","pull_request":{"merged":false}}
        ]"#;
        assert_eq!(page_len(json), 2);
        assert_eq!(parse_issue_list(json).len(), 1);
        assert_eq!(page_len("not json"), 0);
    }

    #[test]
    fn milestone_due_is_normalized_to_a_plain_iso_date() {
        // Gitea returns `due_on` with a real UTC offset, which is where string comparison
        // actually breaks: 2026-08-01T00:00:00+02:00 is BEFORE 2026-07-31T23:00:00Z but
        // sorts after it. Truncating at the T removes the trap.
        let json =
            r#"[{"id":1,"title":"v0.1","state":"open","due_on":"2026-08-01T00:00:00+02:00"}]"#;
        let got = parse_milestones(json);
        assert_eq!(got[0].due.as_deref(), Some("2026-08-01"));
    }
}
