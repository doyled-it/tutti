// SPDX-License-Identifier: AGPL-3.0-or-later
//! The three seams the engine calls down into. Each has an in-memory fake.

use crate::domain::{
    BranchPlan, CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use crate::message::{AgentEvent, AgentOutcome, AgentTask};
use async_trait::async_trait;
use std::path::Path;
use thiserror::Error;
use tokio::sync::mpsc::Sender;

/// Engine-wide error.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("forge: {0}")]
    Forge(String),
    #[error("backend: {0}")]
    Backend(String),
    #[error("routing: {0}")]
    Routing(String),
    #[error("guardrail violated: {0}")]
    Guardrail(String),
    #[error("gate failed:\n{0}")]
    Gate(String),
    /// A capability a forge adapter cannot provide (e.g. a glab/tea adapter that does not
    /// support sub-issues), distinct from a real operational error.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// A token proving an issue is claimed for as long as it is held. There is no
/// `Drop` impl: async release is impossible from `drop`, so on any error after
/// a claim the engine explicitly releases the claim (see the release-on-error
/// path in `run_one`). A stale-issue recovery sweep (reclaiming in-progress
/// issues abandoned by a crash) is a forge-label sweep deferred to the live
/// adapter.
#[derive(Debug)]
pub struct ClaimGuard {
    pub issue: IssueId,
}

impl ClaimGuard {
    pub fn new(issue: IssueId) -> Self {
        Self { issue }
    }
}

/// Runs one headless task. Never pushes, never merges.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    async fn run(
        &self,
        task: AgentTask,
        worktree: &Path,
        events: Sender<AgentEvent>,
    ) -> Result<AgentOutcome>;
}

/// Everything issue/PR/CI. The engine speaks domain types.
#[async_trait]
pub trait Forge: Send + Sync {
    async fn next_ready_issue(&self, filter: &SelectFilter) -> Result<Option<Issue>>;
    /// Flip ready -> in-progress. The label flip is the lock.
    async fn claim(&self, issue: IssueId) -> Result<ClaimGuard>;
    /// Flip in-progress -> ready (failure path).
    async fn release(&self, issue: IssueId) -> Result<()>;
    /// True when `branch` exists on the remote.
    async fn branch_exists(&self, branch: &str) -> Result<bool>;
    /// Create `branch` from `from` on the remote.
    async fn create_branch(&self, branch: &str, from: &str) -> Result<()>;
    /// Push `branch` to origin so a PR can be opened against it.
    async fn push_branch(&self, branch: &str) -> Result<()>;
    async fn open_pr(&self, pr: PrRequest) -> Result<PrHandle>;
    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState>;
    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()>;
    /// Mark done, append decision log, unblock dependents.
    async fn record(&self, issue: IssueId, outcome: &ShipRecord) -> Result<()>;
    /// Reclaim issues abandoned by a crash (in-progress with no open PR/MR -> ready).
    /// The default is a no-op; real adapters override it. Called once before draining.
    async fn recover_stale(&self) -> Result<()> {
        Ok(())
    }

    // --- tracking reads ---
    async fn list_milestones(&self) -> Result<Vec<crate::tracking::Milestone>>;
    /// The issues belonging to a milestone (for a verifiable drain check).
    async fn milestone_children(&self, id: crate::tracking::MilestoneId) -> Result<Vec<Issue>>;
    async fn list_epics(&self) -> Result<Vec<crate::tracking::Epic>>;
    async fn roadmap(&self) -> Result<crate::tracking::Roadmap>;
    // --- tracking writes ---
    async fn create_milestone(
        &self,
        title: &str,
        due: Option<&str>,
        description: &str,
    ) -> Result<crate::tracking::Milestone>;
    async fn close_milestone(&self, id: crate::tracking::MilestoneId) -> Result<()>;
    async fn create_epic(&self, title: &str, body: &str) -> Result<crate::tracking::Epic>;
    async fn link_sub_issue(&self, parent: IssueId, child: IssueId) -> Result<()>;
    /// Create an issue, optionally under a milestone and/or epic. The method the
    /// planner's CreateIssues has always needed.
    async fn create_issue(
        &self,
        new: &crate::message::NewIssue,
        milestone: Option<crate::tracking::MilestoneId>,
        epic: Option<crate::tracking::EpicId>,
    ) -> Result<Issue>;
}

/// Decides one thing: where an issue's work merges. NEVER returns the trunk.
pub trait RoutingStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn target_branch(&self, issue: &Issue) -> Result<BranchPlan>;
}
