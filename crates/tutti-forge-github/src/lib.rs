// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitHub `Forge`: drives `gh` and `git`.
pub mod parse;

use async_trait::async_trait;
use tutti_core::domain::{
    CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use tutti_core::status::{Status, StatusLabels};
use tutti_core::tracking::{Epic, EpicId, Milestone, MilestoneId, Roadmap, TrackState};
use tutti_core::traits::{ClaimGuard, EngineError, Forge, Result};

/// Drives a GitHub repo via `gh` and `git`.
pub struct GitHubForge {
    /// "owner/name".
    pub repo: String,
    /// The labels the engine flips: ready -> in-progress -> done.
    pub status_labels: StatusLabels,
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

    /// Move an issue to `to` by adding its status label and removing the other two.
    /// `gh issue edit --add-label/--remove-label` is idempotent, so removing an
    /// already-absent label is a no-op.
    async fn set_status(&self, issue: IssueId, to: Status) -> Result<()> {
        let n = issue.0.to_string();
        let t = self.status_labels.transition(to);
        let mut args: Vec<&str> = vec![
            "issue",
            "edit",
            &n,
            "--repo",
            &self.repo,
            "--add-label",
            &t.add,
        ];
        for r in &t.remove {
            args.push("--remove-label");
            args.push(r);
        }
        self.gh(&args).await?;
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

    async fn list_issues(&self) -> Result<Vec<Issue>> {
        let json = self
            .gh(&[
                "issue",
                "list",
                "--repo",
                &self.repo,
                "--state",
                "all",
                "--limit",
                "100",
                "--json",
                "number,title,body,labels,milestone",
            ])
            .await?;
        Ok(parse::parse_issue_list(&json))
    }

    // Note: `gh issue edit --add-label/--remove-label` is idempotent and does not error
    // if the issue is already in-progress, so this is NOT the atomic race-guard the design
    // describes; the single-runner `PidLock` provides that guarantee.
    async fn claim(&self, issue: IssueId) -> Result<ClaimGuard> {
        self.set_status(issue, Status::InProgress).await?;
        Ok(ClaimGuard::new(issue))
    }

    async fn release(&self, issue: IssueId) -> Result<()> {
        self.set_status(issue, Status::Ready).await
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
        self.set_status(issue, Status::Done).await
    }

    /// Reclaim issues abandoned by a crash: in-progress issues with no open PR go back
    /// to ready. The CLI calls this once before draining.
    async fn recover_stale(&self) -> Result<()> {
        let json = self
            .gh(&[
                "issue",
                "list",
                "--repo",
                &self.repo,
                "--label",
                &self.status_labels.in_progress,
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

    // --- tracking methods (via `gh api`; parsers are fixture-tested in `parse`) ---

    async fn list_milestones(&self) -> Result<Vec<Milestone>> {
        // `--paginate` merges the per-page arrays into one JSON array. Include closed
        // milestones so the auto-close path can observe an already-closed milestone.
        let endpoint = format!("repos/{}/milestones?state=all", self.repo);
        let json = self.gh(&["api", &endpoint, "--paginate"]).await?;
        Ok(parse::parse_milestones(&json))
    }

    async fn milestone_children(&self, id: MilestoneId) -> Result<Vec<Issue>> {
        // All issues (open and closed) filed under this milestone number.
        let endpoint = format!("repos/{}/issues?milestone={}&state=all", self.repo, id.0);
        let json = self.gh(&["api", &endpoint, "--paginate"]).await?;
        Ok(parse::parse_issue_list(&json))
    }

    async fn list_epics(&self) -> Result<Vec<Epic>> {
        // GitHub has no first-class epic. Treat any issue that HAS sub-issues as an epic.
        // This is a thin read; the planner mainly reasons over milestones.
        let list = self
            .gh(&[
                "issue",
                "list",
                "--repo",
                &self.repo,
                "--limit",
                "100",
                "--json",
                "number,title",
            ])
            .await?;
        #[derive(serde::Deserialize)]
        struct IssueRef {
            number: u64,
            title: String,
        }
        let refs: Vec<IssueRef> = serde_json::from_str(&list).unwrap_or_default();
        let mut epics = Vec::new();
        for r in refs {
            let sub_endpoint = format!("repos/{}/issues/{}/sub_issues", self.repo, r.number);
            let children_json = self.gh(&["api", &sub_endpoint]).await?;
            let children = parse::parse_sub_issues(&children_json);
            if children.is_empty() {
                continue;
            }
            // Progress comes from the issue's own `sub_issues_summary`.
            let issue_endpoint = format!("repos/{}/issues/{}", self.repo, r.number);
            let issue_json = self.gh(&["api", &issue_endpoint]).await?;
            let progress = parse::parse_summary(&issue_json);
            epics.push(Epic {
                id: EpicId(r.number),
                title: r.title,
                children,
                progress,
            });
        }
        Ok(epics)
    }

    async fn roadmap(&self) -> Result<Roadmap> {
        // Open milestones ordered by due date; milestones without a due date sort last.
        let mut milestones: Vec<Milestone> = self
            .list_milestones()
            .await?
            .into_iter()
            .filter(|m| m.state == TrackState::Open)
            .collect();
        milestones.sort_by(|a, b| match (&a.due, &b.due) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        Ok(Roadmap { milestones })
    }

    async fn create_milestone(
        &self,
        title: &str,
        due: Option<&str>,
        description: &str,
    ) -> Result<Milestone> {
        let endpoint = format!("repos/{}/milestones", self.repo);
        let mut args_owned = vec![
            "api".to_string(),
            "--method".to_string(),
            "POST".to_string(),
            endpoint,
            "-f".to_string(),
            format!("title={title}"),
            "-f".to_string(),
            format!("description={description}"),
        ];
        if let Some(d) = due {
            args_owned.push("-f".to_string());
            args_owned.push(format!("due_on={d}"));
        }
        let args: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        let json = self.gh(&args).await?;
        parse::parse_milestone(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created milestone: {json}")))
    }

    async fn close_milestone(&self, id: MilestoneId) -> Result<()> {
        let endpoint = format!("repos/{}/milestones/{}", self.repo, id.0);
        self.gh(&["api", "--method", "PATCH", &endpoint, "-f", "state=closed"])
            .await?;
        Ok(())
    }

    async fn create_epic(&self, title: &str, body: &str) -> Result<Epic> {
        // An epic is a plain issue that will parent sub-issues.
        let endpoint = format!("repos/{}/issues", self.repo);
        let title_arg = format!("title={title}");
        let body_arg = format!("body={body}");
        let json = self
            .gh(&[
                "api", "--method", "POST", &endpoint, "-f", &title_arg, "-f", &body_arg,
            ])
            .await?;
        let issue = parse::parse_created_issue(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created epic: {json}")))?;
        Ok(Epic {
            id: EpicId(issue.id.0),
            title: issue.title,
            children: Vec::new(),
            progress: tutti_core::tracking::Progress::default(),
        })
    }

    async fn link_sub_issue(&self, parent: IssueId, child: IssueId) -> Result<()> {
        // The sub_issues endpoint takes the child's issue DATABASE id (`.id`), not its
        // number, so resolve it first.
        let child_endpoint = format!("repos/{}/issues/{}", self.repo, child.0);
        let child_id = self
            .gh(&["api", &child_endpoint, "--jq", ".id"])
            .await?
            .trim()
            .to_string();
        let parent_endpoint = format!("repos/{}/issues/{}/sub_issues", self.repo, parent.0);
        self.gh(&[
            "api",
            "--method",
            "POST",
            &parent_endpoint,
            "-F",
            &format!("sub_issue_id={child_id}"),
        ])
        .await?;
        Ok(())
    }

    async fn create_issue(
        &self,
        new: &tutti_core::message::NewIssue,
        milestone: Option<MilestoneId>,
        epic: Option<EpicId>,
    ) -> Result<Issue> {
        let endpoint = format!("repos/{}/issues", self.repo);
        let mut args_owned = vec![
            "api".to_string(),
            "--method".to_string(),
            "POST".to_string(),
            endpoint,
            "-f".to_string(),
            format!("title={}", new.title),
            "-f".to_string(),
            format!("body={}", new.body),
        ];
        if let Some(m) = milestone {
            // milestone is an integer field on the create-issue API; -F sends it typed
            // (mirrors the -F sub_issue_id= in link_sub_issue). A raw -f string is rejected.
            args_owned.push("-F".to_string());
            args_owned.push(format!("milestone={}", m.0));
        }
        for label in &new.labels {
            // Labels are low-trust planner input; -f sends them as raw strings so a
            // leading `@` is not treated by gh as a file reference (which -F would do).
            args_owned.push("-f".to_string());
            args_owned.push(format!("labels[]={label}"));
        }
        let args: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        let json = self.gh(&args).await?;
        let issue = parse::parse_created_issue(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created issue: {json}")))?;
        // If an epic parent was given, link the new issue under it as a sub-issue.
        if let Some(epic) = epic {
            self.link_sub_issue(IssueId(epic.0), issue.id).await?;
        }
        Ok(issue)
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
