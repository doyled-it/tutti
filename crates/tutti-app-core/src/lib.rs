// SPDX-License-Identifier: AGPL-3.0-or-later
//! Hermetic logic behind the desktop app: the board model and its assembly from a Forge.
//! No Tauri dependency, so it runs in the fast workspace gate.

use serde::{Deserialize, Serialize};
use tutti_core::config::Config;
use tutti_core::domain::Issue;
use tutti_core::status::StatusLabels;
use tutti_core::tracking::{Milestone, MilestoneId, TrackState};
use tutti_core::traits::{EngineError, Forge, Result};

/// One issue as shown on the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueCard {
    pub id: u64,
    pub title: String,
    pub status: Status,
    pub milestone: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    InProgress,
    Done,
    Other,
}

/// A milestone row for the roadmap rail and the Lanes view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneRow {
    pub id: u64,
    pub title: String,
    pub open: bool,
    pub total: u32,
    pub done: u32,
}

/// The whole board for the selected milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub milestones: Vec<MilestoneRow>,
    pub selected_milestone: Option<u64>,
    pub ready: Vec<IssueCard>,
    pub in_progress: Vec<IssueCard>,
    pub done: Vec<IssueCard>,
}

/// The full detail for the drawer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub milestone: Option<String>,
    pub status: Status,
    pub branch: String,
}

fn classify(issue: &Issue, labels: &StatusLabels) -> Status {
    if issue.has_label(&labels.done) {
        Status::Done
    } else if issue.has_label(&labels.in_progress) {
        Status::InProgress
    } else if issue.has_label(&labels.ready) {
        Status::Ready
    } else {
        Status::Other
    }
}

fn card(issue: &Issue, labels: &StatusLabels) -> IssueCard {
    IssueCard {
        id: issue.id.0,
        title: issue.title.clone(),
        status: classify(issue, labels),
        milestone: issue.milestone.clone(),
    }
}

fn milestone_row(m: &Milestone) -> MilestoneRow {
    MilestoneRow {
        id: m.id.0,
        title: m.title.clone(),
        open: m.state == TrackState::Open,
        total: m.progress.total,
        done: m.progress.done,
    }
}

/// Assemble the board for `select` (default: the earliest open milestone). Reads the
/// milestone list and the selected milestone's children, bucketed by status label.
pub async fn assemble_board(
    forge: &dyn Forge,
    cfg: &Config,
    select: Option<MilestoneId>,
) -> Result<Board> {
    let labels = cfg.status_labels();
    let milestones = forge.list_milestones().await?;
    let rows: Vec<MilestoneRow> = milestones.iter().map(milestone_row).collect();

    // Pick the milestone: the caller's choice, else the first open one, else the first.
    let selected = select.or_else(|| {
        milestones
            .iter()
            .find(|m| m.state == TrackState::Open)
            .or_else(|| milestones.first())
            .map(|m| m.id)
    });

    let (mut ready, mut in_progress, mut done) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(mid) = selected {
        for issue in forge.milestone_children(mid).await? {
            let c = card(&issue, &labels);
            match c.status {
                Status::Ready => ready.push(c),
                Status::InProgress => in_progress.push(c),
                Status::Done => done.push(c),
                Status::Other => ready.push(c), // untriaged shows under Ready
            }
        }
    }

    Ok(Board {
        milestones: rows,
        selected_milestone: selected.map(|m| m.0),
        ready,
        in_progress,
        done,
    })
}

/// Find `id` among all milestones' children and build its drawer detail. Increment 1 has no
/// dedicated single-issue Forge read, so this scans every milestone's children.
pub async fn issue_detail(forge: &dyn Forge, cfg: &Config, id: u64) -> Result<IssueDetail> {
    let labels = cfg.status_labels();
    for m in forge.list_milestones().await? {
        for issue in forge.milestone_children(m.id).await? {
            if issue.id.0 == id {
                return Ok(IssueDetail {
                    id,
                    title: issue.title.clone(),
                    body: issue.body.clone(),
                    labels: issue.labels.clone(),
                    milestone: issue.milestone.clone(),
                    status: classify(&issue, &labels),
                    branch: format!("feat/issue-{id}"),
                });
            }
        }
    }
    Err(EngineError::Forge(format!("issue {id} not found")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_core::config::Config;
    use tutti_core::domain::{CiState, SelectFilter};
    use tutti_core::gate::Gate;
    use tutti_core::message::NewIssue;
    use tutti_core::testing::fake_forge::FakeForge;

    /// Mirrors the `cfg()` test helper in `tutti-core`'s `engine.rs`: default status
    /// labels (`status.status = None`) fall back to the `status:*` convention used below.
    fn cfg() -> Config {
        Config {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "version/v0.1".into(),
            model: "fake".into(),
            max_issues_per_run: 5,
            ci_max_polls: 40,
            poll_delay_secs: 0,
            select: SelectFilter {
                require_label: "status:ready".into(),
                skip_labels: vec!["status:needs-human".into()],
                milestone: None,
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
            status: None,
            forge: Default::default(),
            roles: tutti_core::config::default_roles(),
            merge_mode: tutti_core::domain::MergeMode::Merge,
        }
    }

    /// `FakeForge::milestone_children` resolves through its internal `milestone_of` map,
    /// which is only populated by `create_issue(.., Some(milestone_id), ..)`; issues
    /// preloaded via `FakeForge::new` are never linked. So tests that need
    /// `milestone_children` to see issues must create them through the Forge trait, not
    /// preload them.
    async fn seed_issue(
        forge: &FakeForge,
        milestone: tutti_core::tracking::MilestoneId,
        label: &str,
    ) {
        forge
            .create_issue(
                &NewIssue {
                    title: label.into(),
                    body: String::new(),
                    labels: vec![label.into()],
                },
                Some(milestone),
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn buckets_children_by_status_label() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let m = forge.create_milestone("Phase 1", None, "").await.unwrap();
        seed_issue(&forge, m.id, "status:ready").await;
        seed_issue(&forge, m.id, "status:in-progress").await;
        seed_issue(&forge, m.id, "status:done").await;

        let board = assemble_board(&forge, &cfg(), Some(m.id)).await.unwrap();
        assert_eq!(board.ready.len(), 1);
        assert_eq!(board.in_progress.len(), 1);
        assert_eq!(board.done.len(), 1);
        assert_eq!(board.selected_milestone, Some(m.id.0));
    }

    #[tokio::test]
    async fn issue_detail_finds_issue_across_milestones() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let m = forge.create_milestone("Phase 1", None, "").await.unwrap();
        let created = forge
            .create_issue(
                &NewIssue {
                    title: "do the thing".into(),
                    body: "some body".into(),
                    labels: vec!["status:ready".into()],
                },
                Some(m.id),
                None,
            )
            .await
            .unwrap();

        let detail = issue_detail(&forge, &cfg(), created.id.0).await.unwrap();
        assert_eq!(detail.id, created.id.0);
        assert_eq!(detail.title, "do the thing");
        assert_eq!(detail.body, "some body");
        assert_eq!(detail.status, Status::Ready);
        assert_eq!(detail.milestone.as_deref(), Some("Phase 1"));
        assert_eq!(detail.branch, format!("feat/issue-{}", created.id.0));
    }

    #[tokio::test]
    async fn issue_detail_errors_when_not_found() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let err = issue_detail(&forge, &cfg(), 999).await.unwrap_err();
        assert!(matches!(err, EngineError::Forge(msg) if msg.contains("999")));
    }
}
