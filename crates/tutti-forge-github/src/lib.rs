// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitHub `Forge`: drives `gh` and `git`.
pub mod parse;

use async_trait::async_trait;
use tutti_core::domain::{
    CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use tutti_core::traits::{ClaimGuard, EngineError, Forge, Result};

/// Drives a GitHub repo via `gh` and `git`.
pub struct GitHubForge {
    /// "owner/name".
    pub repo: String,
    /// The label the engine flips: ready -> in-progress -> done.
    pub ready_label: String,
    pub in_progress_label: String,
    pub done_label: String,
    /// Working directory for `git` invocations. `gh` uses `--repo` and is
    /// cwd-independent, but `git ls-remote`/`git push` must run inside the repo.
    pub repo_root: std::path::PathBuf,
}

impl GitHubForge {
    async fn gh(&self, args: &[&str]) -> Result<String> {
        run("gh", args, None).await
    }
    async fn git(&self, args: &[&str]) -> Result<String> {
        run("git", args, Some(&self.repo_root)).await
    }

    /// Reclaim issues abandoned by a crash: in-progress issues with no open PR go back
    /// to ready. The CLI calls this once before draining.
    pub async fn recover_stale(&self) -> Result<()> {
        let json = self
            .gh(&[
                "issue",
                "list",
                "--repo",
                &self.repo,
                "--label",
                &self.in_progress_label,
                "--state",
                "open",
                "--json",
                "number",
                "--jq",
                ".",
            ])
            .await?;
        #[derive(serde::Deserialize)]
        struct N {
            number: u64,
        }
        #[derive(serde::Deserialize)]
        struct Pr {
            #[allow(dead_code)]
            number: u64,
        }
        let issues: Vec<N> = serde_json::from_str(&json).unwrap_or_default();
        for i in issues {
            // Match the PR by its head branch (this repo's convention is
            // `feat/issue-<N>`). `linked:issue-N` is not a valid gh search qualifier,
            // so an empty result there would falsely release issues that already have
            // an open PR, producing duplicate PRs.
            let head = format!("feat/issue-{}", i.number);
            let prs = self
                .gh(&[
                    "pr", "list", "--repo", &self.repo, "--state", "open", "--head", &head,
                    "--json", "number",
                ])
                .await;
            match prs {
                // Spawn/auth failure: cannot tell whether a PR exists, so skip (safest).
                Err(_) => continue,
                Ok(body) => {
                    let open_prs: Vec<Pr> = serde_json::from_str(body.trim()).unwrap_or_default();
                    if open_prs.is_empty() {
                        let _ = self.release(IssueId(i.number)).await;
                    }
                }
            }
        }
        Ok(())
    }
}

/// Run `program` with `args`, erroring on a non-zero exit. Used for the mutating
/// commands where a non-zero status is a genuine failure.
async fn run(program: &str, args: &[&str], cwd: Option<&std::path::Path>) -> Result<String> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd
        .output()
        .await
        .map_err(|e| EngineError::Forge(format!("{program} {:?}: {e}", args)))?;
    if !out.status.success() {
        return Err(EngineError::Forge(format!(
            "{program} {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `program` and return its stdout REGARDLESS of exit status; only Err on a spawn
/// failure. `gh pr checks` exits non-zero by design (8 = pending, 1 = a check failed)
/// while still printing the JSON we need, so the strict `run` would mask real results.
async fn run_capture(program: &str, args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| EngineError::Forge(format!("{program} {:?}: {e}", args)))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[async_trait]
impl Forge for GitHubForge {
    async fn next_ready_issue(&self, filter: &SelectFilter) -> Result<Option<Issue>> {
        let json = self
            .gh(&[
                "issue",
                "list",
                "--repo",
                &self.repo,
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,title,body,labels,milestone",
            ])
            .await?;
        Ok(parse::first_ready_issue(&json, filter))
    }

    // Note: `gh issue edit --add-label/--remove-label` is idempotent and does not error
    // if the issue is already in-progress, so this is NOT the atomic race-guard the design
    // describes; the single-runner `PidLock` provides that guarantee.
    async fn claim(&self, issue: IssueId) -> Result<ClaimGuard> {
        let n = issue.0.to_string();
        self.gh(&[
            "issue",
            "edit",
            &n,
            "--repo",
            &self.repo,
            "--add-label",
            &self.in_progress_label,
            "--remove-label",
            &self.ready_label,
        ])
        .await?;
        Ok(ClaimGuard::new(issue))
    }

    async fn release(&self, issue: IssueId) -> Result<()> {
        let n = issue.0.to_string();
        self.gh(&[
            "issue",
            "edit",
            &n,
            "--repo",
            &self.repo,
            "--add-label",
            &self.ready_label,
            "--remove-label",
            &self.in_progress_label,
        ])
        .await?;
        Ok(())
    }

    async fn branch_exists(&self, branch: &str) -> Result<bool> {
        Ok(self
            .git(&["ls-remote", "--exit-code", "--heads", "origin", branch])
            .await
            .is_ok())
    }

    async fn create_branch(&self, branch: &str, from: &str) -> Result<()> {
        // Seed from the current remote tip, not a possibly-stale remote-tracking ref.
        // Best-effort: if the fetch fails we still try the push against what we have.
        let _ = self.git(&["fetch", "origin", from]).await;
        self.git(&[
            "push",
            "origin",
            &format!("origin/{from}:refs/heads/{branch}"),
        ])
        .await?;
        Ok(())
    }

    async fn push_branch(&self, branch: &str) -> Result<()> {
        // Push the engine-owned feature branch to origin so a PR can be opened
        // against it. force-with-lease because feat branches are engine-owned and
        // recreated with `git worktree add -B`; a stale remote tip must yield to
        // the freshly built local branch, but not clobber an unexpected foreign push.
        self.git(&["push", "-u", "--force-with-lease", "origin", branch])
            .await?;
        Ok(())
    }

    async fn open_pr(&self, pr: PrRequest) -> Result<PrHandle> {
        let out = self
            .gh(&[
                "pr", "create", "--repo", &self.repo, "--base", &pr.base, "--head", &pr.head,
                "--title", &pr.title, "--body", &pr.body,
            ])
            .await?;
        // gh prints the PR URL; the number is the last path segment of the URL line.
        let number = parse::parse_pr_number(&out)
            .ok_or_else(|| EngineError::Forge(format!("could not parse PR number from '{out}'")))?;
        Ok(PrHandle {
            number,
            branch: pr.head,
        })
    }

    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState> {
        // `gh pr checks` exits non-zero by design (8 = pending, 1 = a check failed) yet
        // still prints the JSON on stdout, so read stdout regardless of exit status.
        // Only a genuine spawn failure falls back to Pending.
        let json = run_capture(
            "gh",
            &[
                "pr",
                "checks",
                &pr.number.to_string(),
                "--repo",
                &self.repo,
                "--json",
                "state",
            ],
        )
        .await
        .unwrap_or_else(|_| "[]".into());
        Ok(parse::overall_ci_state(&json))
    }

    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()> {
        let flag = match how {
            MergeMode::Squash => "--squash",
            MergeMode::Merge => "--merge",
            MergeMode::Rebase => "--rebase",
        };
        self.gh(&[
            "pr",
            "merge",
            &pr.number.to_string(),
            "--repo",
            &self.repo,
            flag,
            "--delete-branch",
        ])
        .await?;
        Ok(())
    }

    async fn record(&self, issue: IssueId, _outcome: &ShipRecord) -> Result<()> {
        let n = issue.0.to_string();
        self.gh(&[
            "issue",
            "edit",
            &n,
            "--repo",
            &self.repo,
            "--add-label",
            &self.done_label,
            "--remove-label",
            &self.in_progress_label,
        ])
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse::parse_pr_number;

    #[test]
    fn pr_number_parsing_via_open_pr_helper() {
        // open_pr parses the number from gh's stdout via this shared helper; the full
        // open_pr requires a live gh (Task 9). Cover a plain URL, a trailing newline,
        // and a URL preceding a trailing informational line.
        assert_eq!(
            parse_pr_number("https://github.com/o/r/pull/123"),
            Some(123)
        );
        assert_eq!(
            parse_pr_number("https://github.com/o/r/pull/123\n"),
            Some(123)
        );
        assert_eq!(
            parse_pr_number("Creating pull request...\nhttps://github.com/o/r/pull/9\n"),
            Some(9)
        );
    }
}
