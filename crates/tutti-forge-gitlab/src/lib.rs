// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitLab `Forge`: drives `glab api` (REST v4 + GraphQL) and `git`.
pub mod parse;

use async_trait::async_trait;
use tutti_core::domain::{
    CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use tutti_core::status::{Status, StatusLabels};
use tutti_core::tracking::{Epic, EpicId, Milestone, MilestoneId, Roadmap, TrackState};
use tutti_core::traits::{ClaimGuard, EngineError, Forge, Result};

/// Drives a GitLab project via `glab api` and `git`.
pub struct GitLabForge {
    /// A numeric project id (e.g. "84564301") or a URL-encoded path
    /// ("group%2Fproject"), used verbatim in `projects/{project}/...` endpoints.
    pub project: String,
    /// The status labels the engine flips. On GitLab these are scoped labels
    /// (e.g. "status::ready") so they group under one board column.
    pub status_labels: StatusLabels,
    /// Working directory for `git` invocations (branch push/ls-remote).
    pub repo_root: std::path::PathBuf,
}

impl GitLabForge {
    /// `projects/<project>/<suffix>`, the common REST endpoint prefix.
    fn endpoint(&self, suffix: &str) -> String {
        format!("projects/{}/{}", self.project, suffix.trim_start_matches('/'))
    }

    /// Run `glab api` against a REST endpoint. `fields` become `-f key=value` form
    /// args (used for POST/PUT bodies); GET query params belong in `endpoint`.
    async fn api(&self, method: &str, endpoint: &str, fields: &[(&str, &str)]) -> Result<String> {
        let mut args: Vec<String> = vec!["api".into(), "-X".into(), method.into()];
        for (k, v) in fields {
            args.push("-f".into());
            args.push(format!("{k}={v}"));
        }
        args.push(endpoint.into());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run("glab", &refs, None).await
    }

    async fn git(&self, args: &[&str]) -> Result<String> {
        run("git", args, Some(&self.repo_root)).await
    }

    /// Move an issue to `to` by adding its scoped status label and removing the other
    /// two in one update. GitLab does NOT auto-exclude scoped labels via the API, so the
    /// removal is explicit; removing an absent label is tolerated.
    async fn set_status(&self, issue: IssueId, to: Status) -> Result<()> {
        let t = self.status_labels.transition(to);
        let remove = t.remove.join(",");
        self.api(
            "PUT",
            &self.endpoint(&format!("issues/{}", issue.0)),
            &[("add_labels", &t.add), ("remove_labels", &remove)],
        )
        .await?;
        Ok(())
    }

    /// Resolve the project's parent GROUP id, or None if the project is in a user
    /// namespace (no group -> epics unavailable). GitLab epics are group-level.
    async fn group_id(&self) -> Result<Option<u64>> {
        let json = self
            .api("GET", &format!("projects/{}", self.project), &[])
            .await?;
        Ok(parse::parse_group_id(&json))
    }
}

#[async_trait]
impl Forge for GitLabForge {
    async fn next_ready_issue(&self, filter: &SelectFilter) -> Result<Option<Issue>> {
        let json = self
            .api("GET", &self.endpoint("issues?state=opened&per_page=100"), &[])
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
            .api("GET", &self.endpoint("milestones?state=all&per_page=100"), &[])
            .await?;
        Ok(parse::parse_milestones(&json))
    }

    async fn milestone_children(&self, id: MilestoneId) -> Result<Vec<Issue>> {
        // GitLab filters issues by milestone TITLE, so resolve the title first.
        let mj = self
            .api("GET", &self.endpoint(&format!("milestones/{}", id.0)), &[])
            .await?;
        let title = parse::parse_milestone_title(&mj)
            .ok_or_else(|| EngineError::Forge(format!("milestone {} has no title: {mj}", id.0)))?;
        // URL-encode the title into the query (spaces -> %20). Only encode what a title
        // realistically contains; a minimal space-encode is enough for typical titles.
        let encoded = title.replace(' ', "%20");
        let json = self
            .api(
                "GET",
                &self.endpoint(&format!("issues?state=all&milestone={encoded}&per_page=100")),
                &[],
            )
            .await?;
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
        let mut fields: Vec<(&str, &str)> =
            vec![("title", title), ("description", description)];
        if let Some(d) = due {
            fields.push(("due_date", d));
        }
        let json = self
            .api("POST", &self.endpoint("milestones"), &fields)
            .await?;
        parse::parse_milestone(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created milestone: {json}")))
    }

    async fn close_milestone(&self, id: MilestoneId) -> Result<()> {
        self.api(
            "PUT",
            &self.endpoint(&format!("milestones/{}", id.0)),
            &[("state_event", "close")],
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
        // GitLab takes labels by name as a comma-joined string.
        let labels = new.labels.join(",");
        let milestone_id;
        let mut fields: Vec<(&str, &str)> = vec![
            ("title", &new.title),
            ("description", &new.body),
            ("labels", &labels),
        ];
        if let Some(m) = milestone {
            milestone_id = m.0.to_string();
            fields.push(("milestone_id", &milestone_id));
        }
        let json = self.api("POST", &self.endpoint("issues"), &fields).await?;
        let issue = parse::parse_created_issue(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created issue: {json}")))?;
        // Link under an epic if requested (group-level; errors if epics are unavailable).
        if let Some(e) = epic {
            self.link_sub_issue(IssueId(e.0), issue.id).await?;
        }
        Ok(issue)
    }

    async fn list_epics(&self) -> Result<Vec<Epic>> {
        // Epics live on the parent group. No group -> no epics (graceful empty read).
        let Some(gid) = self.group_id().await? else {
            return Ok(Vec::new());
        };
        // A free-tier group returns 403 here; treat any read failure as "no epics" so a
        // tracking read never hard-fails on epic availability.
        let json = match self
            .api("GET", &format!("groups/{gid}/epics?state=all&per_page=100"), &[])
            .await
        {
            Ok(j) => j,
            Err(_) => return Ok(Vec::new()),
        };
        let mut epics = Vec::new();
        for h in parse::parse_epics(&json) {
            // Children are the epic's issues; progress counts those carrying the done
            // status label (consistent with the milestone drain semantics).
            let cj = self
                .api(
                    "GET",
                    &format!("groups/{gid}/epics/{}/issues?per_page=100", h.iid),
                    &[],
                )
                .await
                .unwrap_or_else(|_| "[]".into());
            let children = parse::parse_issue_list(&cj);
            let done = children
                .iter()
                .filter(|i| i.has_label(&self.status_labels.done))
                .count();
            epics.push(Epic {
                id: EpicId(h.iid),
                title: h.title,
                children: children.iter().map(|i| i.id).collect(),
                progress: tutti_core::tracking::Progress {
                    total: children.len() as u32,
                    done: done as u32,
                },
            });
        }
        Ok(epics)
    }

    async fn create_epic(&self, title: &str, body: &str) -> Result<Epic> {
        let gid = self.group_id().await?.ok_or_else(|| {
            EngineError::Unsupported(
                "GitLab epics require the project to belong to a group".into(),
            )
        })?;
        let json = self
            .api(
                "POST",
                &format!("groups/{gid}/epics"),
                &[("title", title), ("description", body)],
            )
            .await?;
        let h = parse::parse_created_epic(&json)
            .ok_or_else(|| EngineError::Forge(format!("could not parse created epic: {json}")))?;
        Ok(Epic {
            id: EpicId(h.iid),
            title: h.title,
            children: Vec::new(),
            progress: tutti_core::tracking::Progress::default(),
        })
    }

    async fn link_sub_issue(&self, parent: IssueId, child: IssueId) -> Result<()> {
        // `parent` carries the epic iid (the trait reuses IssueId for the epic handle,
        // matching how create_issue calls this). The link endpoint takes the child's
        // GLOBAL issue id, so resolve it from a single-issue GET.
        let gid = self.group_id().await?.ok_or_else(|| {
            EngineError::Unsupported(
                "GitLab epic linking requires the project to belong to a group".into(),
            )
        })?;
        let cj = self
            .api("GET", &self.endpoint(&format!("issues/{}", child.0)), &[])
            .await?;
        let global = parse::parse_issue_global_id(&cj).ok_or_else(|| {
            EngineError::Forge(format!("could not resolve global id for issue {}", child.0))
        })?;
        self.api(
            "POST",
            &format!("groups/{gid}/epics/{}/issues/{global}", parent.0),
            &[],
        )
        .await?;
        Ok(())
    }

    // ... remaining methods added in Task 7 ...
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
