// SPDX-License-Identifier: AGPL-3.0-or-later
//! Greening (retrofit 2b): drive the agent to a green gate per target, in an isolated
//! worktree branch that opens a PR. Orchestration over the AgentBackend, Gate, Forge, and
//! GreenWorkspace seams, so it is testable with no `claude`, git, or network.

use crate::traits::Result;
use std::path::PathBuf;

/// An isolated checkout the greening loop runs in: a git worktree on `branch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenCheckout {
    pub branch: String,
    pub path: PathBuf,
}

/// The change a greening branch introduces vs its base, for the anti-cheat audit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GreenDiff {
    pub changed_files: Vec<PathBuf>,
    pub patch: String,
}

/// A worktree seam for greening: named branches (not issue-coupled like `Workspace`), plus
/// the diff the audit needs and branch-existence for resume.
#[async_trait::async_trait]
pub trait GreenWorkspace: Send + Sync {
    /// True if `branch` already exists locally (drives resume).
    async fn branch_exists(&self, branch: &str) -> Result<bool>;
    /// Create a worktree on `branch`. When `resume` and the branch exists, add the worktree
    /// onto it without resetting; otherwise create/reset `branch` from `base`.
    async fn checkout(&self, branch: &str, base: &str, resume: bool) -> Result<GreenCheckout>;
    /// Stage and commit everything on the checkout's branch. Ok(true) if a commit was made.
    async fn commit_all(&self, co: &GreenCheckout, message: &str) -> Result<bool>;
    /// The diff (changed files + unified patch) of the checkout's branch vs `base`.
    async fn diff(&self, co: &GreenCheckout, base: &str) -> Result<GreenDiff>;
    /// Remove the worktree checkout (the branch persists). Best-effort.
    async fn remove(&self, co: &GreenCheckout) -> Result<()>;
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::sync::Mutex;

    /// A GreenWorkspace over a real tempdir so the real `Gate` can run in it, but with git
    /// operations faked. `existing_branches` seeds resume; `committed` records commits.
    pub struct FakeGreenWorkspace {
        pub dir: std::path::PathBuf,
        pub existing_branches: Vec<String>,
        pub committed: Mutex<Vec<String>>,
        pub diff: Mutex<GreenDiff>,
        pub resumed: Mutex<Vec<String>>,
    }

    impl FakeGreenWorkspace {
        pub fn new(dir: std::path::PathBuf) -> Self {
            Self {
                dir,
                existing_branches: Vec::new(),
                committed: Mutex::new(Vec::new()),
                diff: Mutex::new(GreenDiff::default()),
                resumed: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl GreenWorkspace for FakeGreenWorkspace {
        async fn branch_exists(&self, branch: &str) -> Result<bool> {
            Ok(self.existing_branches.iter().any(|b| b == branch))
        }
        async fn checkout(&self, branch: &str, _base: &str, resume: bool) -> Result<GreenCheckout> {
            if resume && self.existing_branches.iter().any(|b| b == branch) {
                self.resumed.lock().unwrap().push(branch.to_string());
            }
            Ok(GreenCheckout {
                branch: branch.to_string(),
                path: self.dir.clone(),
            })
        }
        async fn commit_all(&self, _co: &GreenCheckout, message: &str) -> Result<bool> {
            self.committed.lock().unwrap().push(message.to_string());
            Ok(true)
        }
        async fn diff(&self, _co: &GreenCheckout, _base: &str) -> Result<GreenDiff> {
            Ok(self.diff.lock().unwrap().clone())
        }
        async fn remove(&self, _co: &GreenCheckout) -> Result<()> {
            Ok(())
        }
    }
}
