// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Gitea/Codeberg `Forge`: drives `tea api` and `git`.
pub mod parse;

use tutti_core::status::StatusLabels;
use tutti_core::traits::{EngineError, Result};

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
