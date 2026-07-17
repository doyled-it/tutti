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
        let json = self.api("GET", &self.endpoint("labels?limit=100"), None).await?;
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

    // ... remaining methods added in Tasks 4-7 ...
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
