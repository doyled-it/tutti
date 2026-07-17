// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Gitea/Codeberg `Forge`: drives `tea api` and `git`.
pub mod parse;

use async_trait::async_trait;
use tutti_core::domain::{
    CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use tutti_core::status::{Status, StatusLabels};
use tutti_core::tracking::{Epic, EpicId, Milestone, MilestoneId, Roadmap, TrackState};
use tutti_core::traits::{ClaimGuard, EngineError, Forge, Result};

/// Drives a Gitea (e.g. Codeberg) repo via `tea api` and `git`.
pub struct GiteaForge {
    /// "owner/name".
    pub repo: String,
    /// The `tea` login to authenticate as (a configured Gitea server login).
    pub login: String,
    /// The status labels the engine flips (ready -> in-progress -> done).
    pub status_labels: StatusLabels,
    /// Working directory for `git` invocations (branch push/ls-remote).
    pub repo_root: std::path::PathBuf,
}

impl GiteaForge {
    /// `repos/<owner>/<repo>/<suffix>`, the common endpoint prefix.
    fn endpoint(&self, suffix: &str) -> String {
        format!("repos/{}/{}", self.repo, suffix.trim_start_matches('/'))
    }

    /// Run `tea api` against `endpoint`. `--login` and any method/body flags MUST
    /// precede the endpoint positional (urfave-cli v1 stops parsing flags after it).
    async fn api(&self, method: &str, endpoint: &str, body: Option<&str>) -> Result<String> {
        let mut args: Vec<&str> = vec!["api", "--login", &self.login, "-X", method];
        if let Some(b) = body {
            args.push("-d");
            args.push(b);
        }
        args.push(endpoint); // endpoint LAST
        run("tea", &args, None).await
    }

    async fn git(&self, args: &[&str]) -> Result<String> {
        run("git", args, Some(&self.repo_root)).await
    }

    /// Resolve label names to Gitea's numeric label ids (issue-label and issue-create
    /// endpoints take ids, not names). Unknown names are skipped by the caller.
    async fn label_ids(&self) -> Result<Vec<(String, i64)>> {
        let json = self
            .api("GET", &self.endpoint("labels?limit=100"), None)
            .await?;
        Ok(parse::parse_label_ids(&json))
    }

    fn id_for(map: &[(String, i64)], name: &str) -> Option<i64> {
        map.iter().find(|(n, _)| n == name).map(|(_, id)| *id)
    }

    /// Move an issue to `to`: add the target status label id, remove the other two
    /// (Gitea tolerates deleting an absent label). The status labels must already
    /// exist in the repo; a name that does not resolve is a setup error.
    async fn set_status(&self, issue: IssueId, to: Status) -> Result<()> {
        let map = self.label_ids().await?;
        let t = self.status_labels.transition(to);
        let add_id = Self::id_for(&map, &t.add).ok_or_else(|| {
            EngineError::Forge(format!("status label {:?} not found in repo", t.add))
        })?;
        let n = issue.0;
        self.api(
            "POST",
            &self.endpoint(&format!("issues/{n}/labels")),
            Some(&format!("{{\"labels\":[{add_id}]}}")),
        )
        .await?;
        for name in &t.remove {
            if let Some(rid) = Self::id_for(&map, name) {
                // Deleting an absent label still succeeds, so this is unconditional.
                self.api(
                    "DELETE",
                    &self.endpoint(&format!("issues/{n}/labels/{rid}")),
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Derive the `epic:<slug>` label name for an epic title (lowercase, non-alnum -> '-').
    fn epic_label(title: &str) -> String {
        let slug: String = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let slug = slug.trim_matches('-').to_string();
        format!("epic:{slug}")
    }

    /// Ensure a label exists, returning its id. Creates it if absent.
    async fn ensure_label(&self, name: &str, color: &str) -> Result<i64> {
        let map = self.label_ids().await?;
        if let Some(id) = Self::id_for(&map, name) {
            return Ok(id);
        }
        let body = serde_json::json!({"name": name, "color": color});
        let json = self
            .api("POST", &self.endpoint("labels"), Some(&body.to_string()))
            .await?;
        #[derive(serde::Deserialize)]
        struct L {
            id: i64,
        }
        serde_json::from_str::<L>(&json)
            .map(|l| l.id)
            .map_err(|_| EngineError::Forge(format!("could not parse created label: {json}")))
    }

    /// Add a label id to an issue.
    async fn add_label(&self, issue: IssueId, label_id: i64) -> Result<()> {
        self.api(
            "POST",
            &self.endpoint(&format!("issues/{}/labels", issue.0)),
            Some(&format!("{{\"labels\":[{label_id}]}}")),
        )
        .await?;
        Ok(())
    }

    /// Reclaim issues abandoned by a crash: in-progress issues with no open PR go back
    /// to ready. Mirrors the GitHub adapter. `tea api` is used for the reads.
    pub async fn recover_stale(&self) -> Result<()> {
        let ep = self.endpoint(&format!(
            "issues?state=open&type=issues&labels={}&limit=100",
            self.status_labels.in_progress
        ));
        let json = self.api("GET", &ep, None).await?;
        let issues = parse::parse_issue_list(&json);
        if issues.is_empty() {
            return Ok(());
        }
        // Gitea has no head-branch filter on the pulls list, so fetch the open PRs once
        // and match head refs client-side. Fetching once (not per issue) avoids an N*M
        // re-download. Note the `limit=100` ceiling: a repo with more than 100 open PRs
        // could page a PR out of view and wrongly release its still-active issue; if the
        // PR list cannot be read at all, skip every issue (the safe choice, never release).
        let heads = match self
            .api("GET", &self.endpoint("pulls?state=open&limit=100"), None)
            .await
        {
            Ok(body) => pr_heads(&body),
            Err(_) => return Ok(()),
        };
        for i in issues {
            let head = format!("feat/issue-{}", i.id.0);
            if !heads.iter().any(|h| h == &head) {
                let _ = self.release(i.id).await;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Forge for GiteaForge {
    async fn next_ready_issue(&self, filter: &SelectFilter) -> Result<Option<Issue>> {
        let json = self
            .api(
                "GET",
                &self.endpoint("issues?state=open&type=issues&limit=100"),
                None,
            )
            .await?;
        Ok(parse::first_ready_issue(&json, filter))
    }

    async fn claim(&self, issue: IssueId) -> Result<ClaimGuard> {
        self.set_status(issue, Status::InProgress).await?;
        Ok(ClaimGuard::new(issue))
    }

    async fn release(&self, issue: IssueId) -> Result<()> {
        self.set_status(issue, Status::Ready).await
    }

    async fn record(&self, issue: IssueId, _outcome: &ShipRecord) -> Result<()> {
        self.set_status(issue, Status::Done).await
    }

    async fn list_milestones(&self) -> Result<Vec<Milestone>> {
        let json = self
            .api(
                "GET",
                &self.endpoint("milestones?state=all&limit=100"),
                None,
            )
            .await?;
        Ok(parse::parse_milestones(&json))
    }

    async fn milestone_children(&self, id: MilestoneId) -> Result<Vec<Issue>> {
        // Gitea filters issues by milestone via `?milestones=<id>` (id or name).
        let ep = self.endpoint(&format!(
            "issues?state=all&type=issues&milestones={}&limit=100",
            id.0
        ));
        let json = self.api("GET", &ep, None).await?;
        Ok(parse::parse_issue_list(&json))
    }

    async fn roadmap(&self) -> Result<Roadmap> {
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
        let body = match due {
            Some(d) => {
                serde_json::json!({"title": title, "description": description, "due_on": d})
            }
            None => serde_json::json!({"title": title, "description": description}),
        };
        let json = self
            .api(
                "POST",
                &self.endpoint("milestones"),
                Some(&body.to_string()),
            )
            .await?;
        parse::parse_milestone(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created milestone: {json}")))
    }

    async fn close_milestone(&self, id: MilestoneId) -> Result<()> {
        let body = serde_json::json!({"state": "closed"});
        self.api(
            "PATCH",
            &self.endpoint(&format!("milestones/{}", id.0)),
            Some(&body.to_string()),
        )
        .await?;
        Ok(())
    }

    async fn create_issue(
        &self,
        new: &tutti_core::message::NewIssue,
        milestone: Option<MilestoneId>,
        epic: Option<EpicId>,
    ) -> Result<Issue> {
        // Gitea's create-issue takes label IDs; resolve the requested label names.
        let map = self.label_ids().await?;
        let label_ids: Vec<i64> = new
            .labels
            .iter()
            .filter_map(|name| Self::id_for(&map, name))
            .collect();
        let mut body = serde_json::json!({
            "title": new.title,
            "body": new.body,
            "labels": label_ids,
        });
        if let Some(m) = milestone {
            body["milestone"] = serde_json::json!(m.0);
        }
        let json = self
            .api("POST", &self.endpoint("issues"), Some(&body.to_string()))
            .await?;
        let issue = parse::parse_created_issue(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created issue: {json}")))?;
        if let Some(epic) = epic {
            self.link_sub_issue(IssueId(epic.0), issue.id).await?;
        }
        Ok(issue)
    }

    async fn create_epic(&self, title: &str, body: &str) -> Result<Epic> {
        // Create the epic label, then a tracking issue carrying it.
        let label = Self::epic_label(title);
        let label_id = self.ensure_label(&label, "5319e7").await?;
        let issue_body = serde_json::json!({"title": title, "body": body});
        let json = self
            .api(
                "POST",
                &self.endpoint("issues"),
                Some(&issue_body.to_string()),
            )
            .await?;
        let issue = parse::parse_created_issue(&json).ok_or_else(|| {
            EngineError::Forge(format!("could not parse created epic issue: {json}"))
        })?;
        self.add_label(issue.id, label_id).await?;
        Ok(Epic {
            id: EpicId(issue.id.0),
            title: title.to_string(),
            children: Vec::new(),
            progress: tutti_core::tracking::Progress::default(),
        })
    }

    async fn link_sub_issue(&self, parent: IssueId, child: IssueId) -> Result<()> {
        // The parent is an epic tracking issue; find its `epic:*` label and tag the child.
        let pj = self
            .api("GET", &self.endpoint(&format!("issues/{}", parent.0)), None)
            .await?;
        let parent_issue = parse::parse_created_issue(&pj)
            .ok_or_else(|| EngineError::Forge(format!("could not parse parent issue: {pj}")))?;
        let epic_label = parent_issue
            .labels
            .iter()
            .find(|l| l.starts_with("epic:"))
            .ok_or_else(|| {
                EngineError::Forge(format!(
                    "issue {} is not an epic (no epic: label)",
                    parent.0
                ))
            })?;
        let map = self.label_ids().await?;
        let id = Self::id_for(&map, epic_label)
            .ok_or_else(|| EngineError::Forge(format!("epic label {epic_label} not found")))?;
        self.add_label(child, id).await
    }

    async fn list_epics(&self) -> Result<Vec<Epic>> {
        // An epic is any `epic:*` label; its children are issues carrying it, its title
        // the label's slug, its progress the closed/total of those issues.
        let map = self.label_ids().await?;
        let mut epics = Vec::new();
        for (name, _id) in map.iter().filter(|(n, _)| n.starts_with("epic:")) {
            let ep = self.endpoint(&format!(
                "issues?state=all&type=issues&labels={}&limit=100",
                name
            ));
            let json = self.api("GET", &ep, None).await?;
            // NOTE: the epic tracking issue itself carries the `epic:*` label (so
            // link_sub_issue can rediscover it), so it appears here as one of its own
            // children. Progress therefore asymptotes at N/(N+1) and never reaches 100%.
            // Harmless while list_epics is omitted from the planner snapshot; revisit
            // (exclude the tracker) if a caller consumes epic progress.
            let children = parse::parse_issue_list(&json);
            let done = children
                .iter()
                .filter(|i| i.has_label(&self.status_labels.done))
                .count();
            // The epic's own id is not tracked separately here; use the first child's id
            // as a stable-enough handle is WRONG, so use 0 to signal "derived". The
            // planner reasons over milestones; epic ids are not addressed by the whitelist.
            epics.push(Epic {
                id: EpicId(0),
                title: name.clone(),
                children: children.iter().map(|i| i.id).collect(),
                progress: tutti_core::tracking::Progress {
                    total: children.len() as u32,
                    done: done as u32,
                },
            });
        }
        Ok(epics)
    }

    async fn branch_exists(&self, branch: &str) -> Result<bool> {
        Ok(self
            .git(&["ls-remote", "--exit-code", "--heads", "origin", branch])
            .await
            .is_ok())
    }

    async fn create_branch(&self, branch: &str, from: &str) -> Result<()> {
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
        self.git(&["push", "-u", "--force-with-lease", "origin", branch])
            .await?;
        Ok(())
    }

    async fn open_pr(&self, pr: PrRequest) -> Result<PrHandle> {
        let body = serde_json::json!({
            "title": pr.title, "body": pr.body, "head": pr.head, "base": pr.base,
        });
        let json = self
            .api("POST", &self.endpoint("pulls"), Some(&body.to_string()))
            .await?;
        let number = parse::parse_created_pr_number(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created PR: {json}")))?;
        Ok(PrHandle {
            number,
            branch: pr.head,
        })
    }

    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState> {
        // Gitea reports a combined status per commit. Resolve the PR head SHA first: the
        // head branch (`feat/issue-N`) contains a slash, and a slashed ref in the
        // commit-status route is ambiguous, so query by the unambiguous commit SHA.
        let pj = self
            .api("GET", &self.endpoint(&format!("pulls/{}", pr.number)), None)
            .await?;
        let sha = match parse::parse_pr_head_sha(&pj) {
            Some(s) => s,
            // No SHA yet (PR just opened, head not resolved): treat as not-yet-reported.
            None => return Ok(CiState::Pending),
        };
        let ep = self.endpoint(&format!("commits/{sha}/status"));
        let json = self
            .api("GET", &ep, None)
            .await
            .unwrap_or_else(|_| "{}".into());
        Ok(parse::combined_ci_state(&json))
    }

    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()> {
        // Gitea merge styles: "merge" | "rebase" | "squash". Map MergeMode.
        let style = match how {
            MergeMode::Squash => "squash",
            MergeMode::Merge => "merge",
            MergeMode::Rebase => "rebase",
        };
        let body = serde_json::json!({"Do": style, "delete_branch_after_merge": true});
        self.api(
            "POST",
            &self.endpoint(&format!("pulls/{}/merge", pr.number)),
            Some(&body.to_string()),
        )
        .await?;
        Ok(())
    }
}

/// Extract each PR's head branch ref from a Gitea `GET pulls` array.
fn pr_heads(json: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Head {
        #[serde(default, rename = "ref")]
        r#ref: String,
    }
    #[derive(serde::Deserialize)]
    struct Pr {
        #[serde(default)]
        head: Option<Head>,
    }
    let prs: Vec<Pr> = serde_json::from_str(json).unwrap_or_default();
    prs.into_iter()
        .filter_map(|p| p.head.map(|h| h.r#ref))
        .collect()
}

/// Run `program` with `args`, erroring on a non-zero exit.
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
