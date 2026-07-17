// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure parsers for GitLab REST JSON (via `glab api`). Kept separate from the shelling
//! so they are unit-testable against captured fixtures.

use serde::Deserialize;
use tutti_core::domain::{CiState, Issue, IssueId, SelectFilter};
use tutti_core::tracking::{Milestone, MilestoneId, Progress, TrackState};

#[derive(Deserialize)]
struct GlMilestoneRef {
    title: String,
}
#[derive(Deserialize)]
struct GlIssue {
    iid: u64,
    title: String,
    // GitLab's issue body field is `description`.
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    milestone: Option<GlMilestoneRef>,
}

fn to_issue(g: GlIssue) -> Issue {
    Issue {
        id: IssueId(g.iid),
        title: g.title,
        body: g.description.unwrap_or_default(),
        labels: g.labels,
        milestone: g.milestone.map(|m| m.title),
    }
}

/// Parse a `GET issues` array and return the first issue matching the filter.
pub fn first_ready_issue(json: &str, filter: &SelectFilter) -> Option<Issue> {
    let issues: Vec<GlIssue> = serde_json::from_str(json).ok()?;
    issues.into_iter().map(to_issue).find(|i| {
        i.has_label(&filter.require_label)
            && !filter.skip_labels.iter().any(|s| i.has_label(s))
            && filter
                .milestone
                .as_ref()
                .is_none_or(|m| i.milestone.as_ref() == Some(m))
    })
}

/// Parse a `GET issues` array into `Issue`s. GitLab keeps issues and merge requests on
/// separate endpoints, so there is no pull-request element to filter.
pub fn parse_issue_list(json: &str) -> Vec<Issue> {
    let issues: Vec<GlIssue> = serde_json::from_str(json).unwrap_or_default();
    issues.into_iter().map(to_issue).collect()
}

/// Parse a single issue object (create/get response) into an `Issue`.
pub fn parse_created_issue(json: &str) -> Option<Issue> {
    let g: GlIssue = serde_json::from_str(json).ok()?;
    Some(to_issue(g))
}

#[derive(Deserialize)]
struct GlMilestone {
    id: u64,
    title: String,
    state: String,
    #[serde(default)]
    due_date: Option<String>,
}

fn milestone_from(m: GlMilestone) -> Milestone {
    Milestone {
        id: MilestoneId(m.id),
        title: m.title,
        // GitLab milestone state is active|closed. No issue counters live on the
        // milestone object, so progress is derived by milestone_children, not here.
        state: match m.state.as_str() {
            "closed" => TrackState::Closed,
            _ => TrackState::Open,
        },
        due: m.due_date,
        progress: Progress::default(),
    }
}

/// Parse a `GET milestones` array into `Milestone`s (progress left at default; the
/// engine computes a verified drain via milestone_children).
pub fn parse_milestones(json: &str) -> Vec<Milestone> {
    let raw: Vec<GlMilestone> = serde_json::from_str(json).unwrap_or_default();
    raw.into_iter().map(milestone_from).collect()
}

/// Parse a single milestone object (create/PUT response) into a `Milestone`.
pub fn parse_milestone(json: &str) -> Option<Milestone> {
    let raw: GlMilestone = serde_json::from_str(json).ok()?;
    Some(milestone_from(raw))
}

/// Read a milestone's title from a `GET milestones/{id}` response, needed because
/// GitLab filters issues by milestone TITLE, not id.
pub fn parse_milestone_title(json: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct M {
        title: String,
    }
    serde_json::from_str::<M>(json).ok().map(|m| m.title)
}

/// Parse the `iid` from a `POST merge_requests` create response.
pub fn parse_created_mr_iid(json: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Mr {
        iid: u64,
    }
    serde_json::from_str::<Mr>(json).ok().map(|m| m.iid)
}

/// Resolve a project's parent GROUP id from a `GET projects/{id}` response. Returns
/// Some(group_id) only when the namespace is a group (epics are group-level); None for a
/// user namespace (no group, so epics are unavailable).
pub fn parse_group_id(json: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Ns {
        #[serde(default)]
        kind: String,
        #[serde(default)]
        id: u64,
    }
    #[derive(Deserialize)]
    struct Project {
        #[serde(default)]
        namespace: Option<Ns>,
    }
    let p: Project = serde_json::from_str(json).ok()?;
    let ns = p.namespace?;
    (ns.kind == "group").then_some(ns.id)
}

/// A group epic header from `GET groups/{id}/epics` or a create response: its `iid`
/// (mapped to EpicId) and title. Children/progress are fetched separately.
pub struct EpicHeader {
    pub iid: u64,
    pub title: String,
}

/// Parse a `GET groups/{id}/epics` array into epic headers.
pub fn parse_epics(json: &str) -> Vec<EpicHeader> {
    #[derive(Deserialize)]
    struct E {
        iid: u64,
        title: String,
    }
    let raw: Vec<E> = serde_json::from_str(json).unwrap_or_default();
    raw.into_iter()
        .map(|e| EpicHeader {
            iid: e.iid,
            title: e.title,
        })
        .collect()
}

/// Parse a single epic object (a `POST groups/{id}/epics` create response) into a header.
pub fn parse_created_epic(json: &str) -> Option<EpicHeader> {
    #[derive(Deserialize)]
    struct E {
        iid: u64,
        title: String,
    }
    serde_json::from_str::<E>(json)
        .ok()
        .map(|e| EpicHeader {
            iid: e.iid,
            title: e.title,
        })
}

/// Read an issue's GLOBAL id from a `GET projects/{id}/issues/{iid}` response. GitLab's
/// epic-issue link endpoint takes the global id, not the iid.
pub fn parse_issue_global_id(json: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct I {
        id: u64,
    }
    serde_json::from_str::<I>(json).ok().map(|i| i.id)
}

/// Map a merge request's `pipeline.status` (from `GET merge_requests/{iid}`) to a
/// `CiState`. A missing pipeline is treated as not-yet-reported (Pending).
pub fn mr_ci_state(json: &str) -> CiState {
    #[derive(Deserialize)]
    struct Pipeline {
        #[serde(default)]
        status: String,
    }
    #[derive(Deserialize)]
    struct Mr {
        #[serde(default)]
        pipeline: Option<Pipeline>,
    }
    let mr: Mr = match serde_json::from_str(json) {
        Ok(m) => m,
        Err(_) => return CiState::Pending,
    };
    match mr.pipeline {
        None => CiState::Pending,
        Some(p) => match p.status.as_str() {
            "success" => CiState::Pass,
            "failed" | "canceled" => CiState::Fail,
            _ => CiState::Pending, // running, pending, created, manual, skipped
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> SelectFilter {
        SelectFilter {
            require_label: "status::ready".into(),
            skip_labels: vec!["status::needs-human".into()],
            milestone: None,
        }
    }

    #[test]
    fn picks_first_ready_skipping_needs_human() {
        let json = r#"[
          {"iid":1,"title":"a","description":"","labels":["status::ready","status::needs-human"],"milestone":null},
          {"iid":2,"title":"b","description":"x","labels":["status::ready"],"milestone":{"title":"Phase 1"}}
        ]"#;
        let issue = first_ready_issue(json, &filter()).unwrap();
        assert_eq!(issue.id.0, 2);
        assert_eq!(issue.milestone.as_deref(), Some("Phase 1"));
    }

    #[test]
    fn issue_uses_iid_and_description_as_body() {
        // Real captured single-issue response: `glab api projects/84564301/issues/1`.
        let json = include_str!("../tests/fixtures/issue.json");
        let issue = parse_created_issue(json).unwrap();
        assert_eq!(issue.id, IssueId(1));
        assert_eq!(issue.title, "first sandbox issue");
        assert_eq!(issue.body, "hello");
        assert!(issue.has_label("status::ready"));
        assert_eq!(issue.milestone.as_deref(), Some("Phase 1"));
    }

    #[test]
    fn milestones_map_active_to_open() {
        // Real captured response: `glab api "projects/84564301/milestones?state=all"`.
        let json = include_str!("../tests/fixtures/milestones.json");
        let ms = parse_milestones(json);
        let phase1 = ms.iter().find(|m| m.title == "Phase 1").unwrap();
        assert_eq!(phase1.state, TrackState::Open);
        assert_eq!(phase1.id, MilestoneId(7520655));
    }

    #[test]
    fn ci_states_map() {
        assert_eq!(mr_ci_state(r#"{"pipeline":{"status":"success"}}"#), CiState::Pass);
        assert_eq!(mr_ci_state(r#"{"pipeline":{"status":"failed"}}"#), CiState::Fail);
        assert_eq!(mr_ci_state(r#"{"pipeline":{"status":"running"}}"#), CiState::Pending);
        assert_eq!(mr_ci_state(r#"{"pipeline":null}"#), CiState::Pending);
    }

    #[test]
    fn group_id_only_for_group_namespace() {
        // Real captured sandbox project is under a USER namespace: no group.
        let json = include_str!("../tests/fixtures/project.json");
        assert_eq!(parse_group_id(json), None);
        // A group namespace resolves to the group id.
        assert_eq!(
            parse_group_id(r#"{"id":1,"namespace":{"kind":"group","id":42}}"#),
            Some(42)
        );
    }

    #[test]
    fn epics_and_children_parse() {
        // epics.json / epic_issues.json are hand-authored from GitLab's documented epic
        // REST shapes (see docs/plans/2026-07-17-tutti-slice3b-glab.md, Task 2 Step 1):
        // the spike-tier sandbox has no Premium group, so these could not be captured
        // live.
        let epics = parse_epics(include_str!("../tests/fixtures/epics.json"));
        assert_eq!(epics.len(), 1);
        assert_eq!(epics[0].iid, 4);
        // Epic children are ordinary issue objects, read by parse_issue_list.
        let children = parse_issue_list(include_str!("../tests/fixtures/epic_issues.json"));
        assert_eq!(children.len(), 1);
        assert!(children[0].has_label("status::done"));
        // Epic-issue linking uses the global id, resolved from a single-issue GET.
        assert_eq!(
            parse_issue_global_id(r#"{"iid":13,"id":55}"#),
            Some(55)
        );
    }
}
