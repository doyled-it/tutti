// SPDX-License-Identifier: AGPL-3.0-or-later
//! The workspace seam: isolate each issue's work on its own branch and directory.
//! `tutti-core` defines the trait and a no-op fake; the real git implementation
//! lives in the `tutti-git` crate so this crate stays free of a git dependency.

use crate::domain::IssueId;
use crate::traits::Result;
use async_trait::async_trait;
use std::path::PathBuf;

/// A live isolated workspace for one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceHandle {
    pub issue: IssueId,
    pub path: PathBuf,
    pub branch: String,
}

/// Creates and tears down an isolated working directory per issue.
#[async_trait]
pub trait Workspace: Send + Sync {
    /// Create an isolated workspace for `issue` on a fresh `feat/issue-N` branch
    /// based on `base`. Returns the directory the agent should run in.
    async fn create(&self, issue: IssueId, base: &str) -> Result<WorkspaceHandle>;
    /// Remove the workspace after the issue reaches a terminal state (best-effort).
    async fn remove(&self, handle: &WorkspaceHandle) -> Result<()>;
    /// Prune any Tutti workspaces left behind by a crashed run.
    async fn prune(&self) -> Result<()>;
}

/// A no-op workspace for offline tests: reports a directory but touches no disk.
pub struct NoopWorkspace {
    root: PathBuf,
}

impl NoopWorkspace {
    /// Build a no-op workspace whose handles all point at `root`.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Default for NoopWorkspace {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
        }
    }
}

#[async_trait]
impl Workspace for NoopWorkspace {
    async fn create(&self, issue: IssueId, _base: &str) -> Result<WorkspaceHandle> {
        Ok(WorkspaceHandle {
            issue,
            path: self.root.clone(),
            branch: format!("feat/issue-{}", issue.0),
        })
    }
    async fn remove(&self, _handle: &WorkspaceHandle) -> Result<()> {
        Ok(())
    }
    async fn prune(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_create_reports_a_branch_and_path() {
        let ws = NoopWorkspace::new(PathBuf::from("/tmp/x"));
        let h = ws.create(IssueId(7), "main").await.unwrap();
        assert_eq!(h.branch, "feat/issue-7");
        assert_eq!(h.path, PathBuf::from("/tmp/x"));
        ws.remove(&h).await.unwrap();
        ws.prune().await.unwrap();
    }
}
