// SPDX-License-Identifier: AGPL-3.0-or-later
//! A real-git `GreenWorkspace`: named worktree branches under `.worktrees/`, plus the diff
//! the greening anti-cheat needs.

use std::path::PathBuf;
use tutti_core::greening::{GreenCheckout, GreenDiff, GreenWorkspace};
use tutti_core::traits::{EngineError, Result};

/// Isolates a greening branch in a git worktree rooted at `repo_root`.
pub struct GitGreenWorkspace {
    repo_root: PathBuf,
    /// Serializes worktree-admin mutations (`worktree add` / `worktree remove`): git's
    /// `.git/worktrees` bookkeeping is not safe for concurrent targets to contend on.
    lock: tokio::sync::Mutex<()>,
}

impl GitGreenWorkspace {
    /// Build a green workspace manager for the git repo at `repo_root`.
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    async fn git(&self, args: &[&str]) -> Result<std::process::Output> {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .await
            .map_err(|e| EngineError::Forge(format!("git {args:?}: {e}")))
    }

    fn checkout_path(&self, branch: &str) -> PathBuf {
        let safe = branch.replace('/', "-"); // green/python -> green-python
        self.repo_root.join(".worktrees").join(safe)
    }
}

#[async_trait::async_trait]
impl GreenWorkspace for GitGreenWorkspace {
    async fn branch_exists(&self, branch: &str) -> Result<bool> {
        let out = self
            .git(&[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .await?;
        Ok(out.status.success())
    }

    async fn checkout(&self, branch: &str, base: &str, resume: bool) -> Result<GreenCheckout> {
        let path = self.checkout_path(branch);
        let path_str = path.to_string_lossy().to_string();
        let base_ref = if resume && self.branch_exists(branch).await? {
            None
        } else {
            Some(if self.branch_exists(base).await? {
                base.to_string()
            } else {
                format!("origin/{base}")
            })
        };
        {
            let _guard = self.lock.lock().await;
            // Drop any stale worktree at that path first (best-effort).
            let _ = self
                .git(&["worktree", "remove", "--force", &path_str])
                .await;
            match &base_ref {
                None => {
                    let out = self.git(&["worktree", "add", &path_str, branch]).await?;
                    if !out.status.success() {
                        return Err(EngineError::Forge(format!(
                            "git worktree add (resume) failed: {}",
                            String::from_utf8_lossy(&out.stderr)
                        )));
                    }
                }
                Some(base_ref) => {
                    let out = self
                        .git(&["worktree", "add", "-B", branch, &path_str, base_ref])
                        .await?;
                    if !out.status.success() {
                        return Err(EngineError::Forge(format!(
                            "git worktree add failed: {}",
                            String::from_utf8_lossy(&out.stderr)
                        )));
                    }
                }
            }
        }
        Ok(GreenCheckout {
            branch: branch.to_string(),
            path,
        })
    }

    async fn commit_all(&self, co: &GreenCheckout, message: &str) -> Result<bool> {
        let wt = co.path.to_string_lossy().to_string();
        let _ = self.git(&["-C", &wt, "add", "-A"]).await?;
        let staged = self
            .git(&["-C", &wt, "diff", "--cached", "--quiet"])
            .await?;
        if staged.status.success() {
            return Ok(false); // nothing staged
        }
        let out = self
            .git(&[
                "-C",
                &wt,
                "-c",
                "user.name=tutti",
                "-c",
                "user.email=tutti@local",
                "commit",
                "-m",
                message,
            ])
            .await?;
        Ok(out.status.success())
    }

    async fn has_commits(&self, co: &GreenCheckout, base: &str) -> Result<bool> {
        let wt = co.path.to_string_lossy().to_string();
        // Count commits on the checkout's branch that are not reachable from base. Non-zero
        // means the branch is ahead (e.g. the agent committed its own fix), so it is worth
        // shipping even if `commit_all` found a clean tree.
        let out = self
            .git(&["-C", &wt, "rev-list", "--count", &format!("{base}..HEAD")])
            .await?;
        if !out.status.success() {
            return Ok(false);
        }
        let n: u64 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(n > 0)
    }

    async fn diff(&self, co: &GreenCheckout, base: &str) -> Result<GreenDiff> {
        let wt = co.path.to_string_lossy().to_string();
        let names = self.git(&["-C", &wt, "diff", "--name-only", base]).await?;
        let changed_files = String::from_utf8_lossy(&names.stdout)
            .lines()
            .map(PathBuf::from)
            .collect();
        let patch = self.git(&["-C", &wt, "diff", base]).await?;
        Ok(GreenDiff {
            changed_files,
            patch: String::from_utf8_lossy(&patch.stdout).into_owned(),
        })
    }

    async fn remove(&self, co: &GreenCheckout) -> Result<()> {
        let path_str = co.path.to_string_lossy().to_string();
        let _guard = self.lock.lock().await;
        let _ = self
            .git(&["worktree", "remove", "--force", &path_str])
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "live: needs git on PATH"]
    async fn checkout_commit_diff_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
        };
        run(&["init", "-q"]);
        run(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        run(&["branch", "staging"]);
        let ws = GitGreenWorkspace::new(root.clone());
        assert!(!ws.branch_exists("green/x").await.unwrap());
        let co = ws.checkout("green/x", "staging", false).await.unwrap();
        assert!(ws.branch_exists("green/x").await.unwrap());
        assert!(
            !ws.has_commits(&co, "staging").await.unwrap(),
            "fresh branch is not ahead"
        );
        std::fs::write(co.path.join("f.txt"), "hi\n").unwrap();
        assert!(ws.commit_all(&co, "add f").await.unwrap());
        assert!(
            ws.has_commits(&co, "staging").await.unwrap(),
            "branch is ahead after a commit"
        );
        let diff = ws.diff(&co, "staging").await.unwrap();
        assert!(diff.changed_files.iter().any(|p| p.ends_with("f.txt")));
        ws.remove(&co).await.unwrap();
    }
}
