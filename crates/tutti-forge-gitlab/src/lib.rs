// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitLab `Forge`: drives `glab api` (REST v4 + GraphQL) and `git`.
pub mod parse;

use tutti_core::status::StatusLabels;
use tutti_core::traits::{EngineError, Result};

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
