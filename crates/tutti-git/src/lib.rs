// SPDX-License-Identifier: AGPL-3.0-or-later
//! A `Workspace` backed by real `git worktree`, isolating each issue on its own
//! branch and directory under `<repo>/.worktrees/tutti-issue-N`.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tutti_core::domain::IssueId;
use tutti_core::traits::{EngineError, Result};
use tutti_core::workspace::{Workspace, WorkspaceHandle};

/// Isolates issue work in git worktrees rooted at `repo_root`.
pub struct GitWorkspace {
    repo_root: PathBuf,
}

impl GitWorkspace {
    /// Build a workspace manager for the git repo at `repo_root`.
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
        }
    }

    fn worktree_path(&self, issue: IssueId) -> PathBuf {
        self.repo_root
            .join(".worktrees")
            .join(format!("tutti-issue-{}", issue.0))
    }

    async fn git(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| EngineError::Forge(format!("git {:?}: {e}", args)))?;
        if !out.status.success() {
            return Err(EngineError::Forge(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Resolve `base` to a commit-ish that exists locally for `git worktree add`.
    ///
    /// The engine picks `base` from `Forge::branch_exists`, which is remote truth
    /// ("exists on origin"). `git worktree add` resolves its base as a local
    /// commit-ish, so on a fresh clone an integration branch that exists on origin
    /// but not locally would fail or resolve to a stale ref. Prefer the local ref,
    /// fall back to `origin/<base>`, and otherwise error asking for a fetch.
    async fn resolve_base(&self, base: &str) -> Result<String> {
        if self
            .git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{base}^{{commit}}"),
            ])
            .await
            .is_ok()
        {
            return Ok(base.to_string());
        }
        let origin = format!("origin/{base}");
        if self
            .git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{origin}^{{commit}}"),
            ])
            .await
            .is_ok()
        {
            return Ok(origin);
        }
        Err(EngineError::Forge(format!(
            "base ref '{base}' not found locally or as origin/{base}; fetch first"
        )))
    }
}

#[async_trait]
impl Workspace for GitWorkspace {
    async fn create(&self, issue: IssueId, base: &str) -> Result<WorkspaceHandle> {
        let path = self.worktree_path(issue);
        let branch = format!("feat/issue-{}", issue.0);
        let path_str = path.to_string_lossy().into_owned();
        // Remove any stale worktree at that path first (best-effort).
        let _ = self
            .git(&["worktree", "remove", "--force", &path_str])
            .await;
        // Resolve `base` to a ref that exists locally (it may be remote-only on a
        // fresh clone) before handing it to git.
        let base_ref = self.resolve_base(base).await?;
        // git worktree add -B <branch> <path> <base>
        // `-B` creates the branch or resets it to `base` if it already exists
        // (e.g. left over from a prior create/prune cycle for the same issue),
        // which keeps `create` idempotent per issue.
        self.git(&["worktree", "add", "-B", &branch, &path_str, &base_ref])
            .await?;
        Ok(WorkspaceHandle {
            issue,
            path,
            branch,
        })
    }

    async fn remove(&self, handle: &WorkspaceHandle) -> Result<()> {
        let path_str = handle.path.to_string_lossy().into_owned();
        self.git(&["worktree", "remove", "--force", &path_str])
            .await?;
        Ok(())
    }

    /// Single-runner startup sweep: force-removes ALL `tutti-issue-*` worktrees.
    /// This is NOT concurrency-safe and must not run while another runner holds an
    /// issue worktree; it would tear that runner's workspace out from under it.
    async fn prune(&self) -> Result<()> {
        // Drop bookkeeping for any manually deleted worktrees.
        self.git(&["worktree", "prune"]).await?;
        // Remove any leftover Tutti worktrees from a crashed run.
        let list = self.git(&["worktree", "list", "--porcelain"]).await?;
        let mut to_remove = Vec::new();
        for line in list.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                if Path::new(p)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("tutti-issue-"))
                {
                    to_remove.push(p.to_string());
                }
            }
        }
        for p in to_remove {
            let _ = self.git(&["worktree", "remove", "--force", &p]).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run a git command in `root`, asserting it succeeds.
    async fn run_git(root: &Path, args: Vec<&str>) {
        let ok = Command::new("git")
            .args(&args)
            .current_dir(root)
            .output()
            .await
            .unwrap();
        assert!(
            ok.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&ok.stderr)
        );
    }

    /// Create a temp git repo with one commit on `main` and return its temp dir.
    async fn temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run_git(root, vec!["init", "-b", "main"]).await;
        run_git(root, vec!["config", "user.email", "t@t.t"]).await;
        run_git(root, vec!["config", "user.name", "t"]).await;
        std::fs::write(root.join("README.md"), "x").unwrap();
        run_git(root, vec!["add", "."]).await;
        run_git(root, vec!["commit", "-m", "init"]).await;
        dir
    }

    #[tokio::test]
    async fn create_makes_a_worktree_on_a_new_branch() {
        let dir = temp_repo().await;
        let ws = GitWorkspace::new(dir.path());
        let h = ws.create(IssueId(3), "main").await.unwrap();
        assert!(h.path.exists(), "worktree dir should exist");
        assert!(
            h.path.join("README.md").exists(),
            "base content should be checked out"
        );
        assert_eq!(h.branch, "feat/issue-3");
    }

    #[tokio::test]
    async fn remove_deletes_the_worktree() {
        let dir = temp_repo().await;
        let ws = GitWorkspace::new(dir.path());
        let h = ws.create(IssueId(4), "main").await.unwrap();
        ws.remove(&h).await.unwrap();
        assert!(!h.path.exists(), "worktree dir should be gone");
    }

    #[tokio::test]
    async fn prune_removes_leftover_tutti_worktrees() {
        let dir = temp_repo().await;
        let ws = GitWorkspace::new(dir.path());
        let h = ws.create(IssueId(5), "main").await.unwrap();
        // Simulate a crash: delete the dir out from under git without `worktree remove`.
        std::fs::remove_dir_all(&h.path).unwrap();
        ws.prune().await.unwrap();
        // A second create for the same issue must now succeed cleanly.
        let h2 = ws.create(IssueId(5), "main").await.unwrap();
        assert!(h2.path.exists());
    }

    #[tokio::test]
    async fn create_resets_branch_to_base_on_recreate() {
        let dir = temp_repo().await;
        let ws = GitWorkspace::new(dir.path());
        let h = ws.create(IssueId(9), "main").await.unwrap();
        // Commit a new file inside the issue worktree.
        std::fs::write(h.path.join("scratch.txt"), "work").unwrap();
        run_git(&h.path, vec!["add", "."]).await;
        run_git(&h.path, vec!["commit", "-m", "wip"]).await;
        ws.remove(&h).await.unwrap();
        // Recreating from base must reset the branch (`-B`), dropping that commit.
        let h2 = ws.create(IssueId(9), "main").await.unwrap();
        assert!(
            !h2.path.join("scratch.txt").exists(),
            "recreate must reset the branch to base, dropping prior commits"
        );
    }

    #[tokio::test]
    async fn prune_leaves_non_tutti_worktrees_intact() {
        let dir = temp_repo().await;
        let ws = GitWorkspace::new(dir.path());
        // A worktree that is not a Tutti issue worktree must survive prune.
        let other = dir.path().join("other-wt");
        let other_str = other.to_string_lossy().into_owned();
        run_git(
            dir.path(),
            vec!["worktree", "add", &other_str, "-b", "other", "main"],
        )
        .await;
        // A Tutti worktree whose dir crashed away.
        let h = ws.create(IssueId(11), "main").await.unwrap();
        std::fs::remove_dir_all(&h.path).unwrap();
        ws.prune().await.unwrap();
        assert!(other.exists(), "prune must not touch non-Tutti worktrees");
        // The Tutti worktree slot is reclaimed, so a fresh create succeeds.
        let h2 = ws.create(IssueId(11), "main").await.unwrap();
        assert!(h2.path.exists());
    }

    #[tokio::test]
    async fn create_errors_when_base_missing() {
        let dir = temp_repo().await;
        let ws = GitWorkspace::new(dir.path());
        let err = ws
            .create(IssueId(12), "nonexistent-branch")
            .await
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("nonexistent-branch"),
            "error should name the missing base: {msg}"
        );
    }
}
