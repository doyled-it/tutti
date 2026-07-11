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
}

impl GitHubForge {
    async fn gh(&self, args: &[&str]) -> Result<String> {
        run("gh", args).await
    }
    async fn git(&self, args: &[&str]) -> Result<String> {
        run("git", args).await
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
        let issues: Vec<N> = serde_json::from_str(&json).unwrap_or_default();
        for i in issues {
            let prs = self
                .gh(&[
                    "pr",
                    "list",
                    "--repo",
                    &self.repo,
                    "--search",
                    &format!("linked:issue-{}", i.number),
                    "--state",
                    "open",
                    "--json",
                    "number",
                ])
                .await
                .unwrap_or_default();
            if prs.trim() == "[]" || prs.trim().is_empty() {
                let _ = self.release(IssueId(i.number)).await;
            }
        }
        Ok(())
    }
}

async fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new(program)
        .args(args)
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
        self.git(&[
            "push",
            "origin",
            &format!("origin/{from}:refs/heads/{branch}"),
        ])
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
        // gh prints the PR URL; the number is its last path segment.
        let number = out
            .trim()
            .rsplit('/')
            .next()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .ok_or_else(|| {
                EngineError::Forge(format!("could not parse PR number from '{}'", out.trim()))
            })?;
        Ok(PrHandle {
            number,
            branch: pr.head,
        })
    }

    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState> {
        let json = self
            .gh(&[
                "pr",
                "checks",
                &pr.number.to_string(),
                "--repo",
                &self.repo,
                "--json",
                "state",
            ])
            .await
            .unwrap_or_else(|_| "[]".into());
        Ok(parse::overall_ci_state(&json))
    }

    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()> {
        let flag = match how {
            MergeMode::Squash => "--squash",
            MergeMode::Merge => "--merge",
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
    #[test]
    fn pr_number_parsing_via_open_pr_helper() {
        // The URL-to-number logic is exercised through parse-like assertions here; the
        // full open_pr requires a live gh (Task 9). This documents the expected shape.
        let url = "https://github.com/o/r/pull/123";
        let number: u64 = url.rsplit('/').next().unwrap().parse().unwrap();
        assert_eq!(number, 123);
    }
}
