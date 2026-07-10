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
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// A disarm marker proving an issue is claimed. There is no `Drop` impl: async
/// release is impossible from `drop`, so on any error after a claim the engine
/// explicitly releases the claim (see the release-on-error path in `run_one`).
/// A stale-issue recovery sweep (reclaiming in-progress issues abandoned by a
/// crash) is deferred to the live-forge adapter.
#[derive(Debug)]
pub struct ClaimGuard {
    pub issue: IssueId,
    /// Read by the future recover sweep, not in slice 1; kept as the disarm state.
    #[allow(dead_code)]
    armed: bool,
}

impl ClaimGuard {
    pub fn new(issue: IssueId) -> Self {
        Self { issue, armed: true }
    }
    /// Disarm the guard once the claim has been resolved (shipped or released).
    pub fn disarm(&mut self) {
        self.armed = false;
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
    async fn open_pr(&self, pr: PrRequest) -> Result<PrHandle>;
    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState>;
    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()>;
    /// Mark done, append decision log, unblock dependents.
    async fn record(&self, issue: IssueId, outcome: &ShipRecord) -> Result<()>;
}

/// Decides one thing: where an issue's work merges. NEVER returns the trunk.
pub trait RoutingStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn target_branch(&self, issue: &Issue) -> Result<BranchPlan>;
}
