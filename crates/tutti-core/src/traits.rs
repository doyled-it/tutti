// SPDX-License-Identifier: AGPL-3.0-or-later
//! The three seams the engine calls down into. Each has an in-memory fake.

use crate::domain::{
    BranchPlan, CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use crate::message::{AgentEvent, AgentOutcome, AgentTask};
use async_trait::async_trait;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

/// RAII marker proving an issue is claimed. See the module note on async Drop.
#[derive(Debug)]
pub struct ClaimGuard {
    pub issue: IssueId,
    released: Arc<AtomicBool>,
}

impl ClaimGuard {
    pub fn new(issue: IssueId) -> Self {
        Self {
            issue,
            released: Arc::new(AtomicBool::new(false)),
        }
    }
    /// Mark the guard as explicitly released (call after `forge.release`).
    pub fn mark_released(&self) {
        self.released.store(true, Ordering::SeqCst);
    }
    /// For tests: was this guard released before drop?
    pub fn was_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
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
