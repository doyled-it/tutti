// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build the concrete engine adapters (a forge chosen by kind + Claude backend + git
//! workspace) from config.

use std::path::PathBuf;
use tutti_backend_claude::ClaudeBackend;
use tutti_core::config::{Config, ForgeKind};
use tutti_core::traits::{EngineError, Forge, Result};
use tutti_forge_gitea::GiteaForge;
use tutti_forge_github::GitHubForge;
use tutti_forge_gitlab::GitLabForge;
use tutti_git::GitWorkspace;

/// The concrete adapter set the CLI runs with. The forge is dynamic so one build path
/// serves GitHub, Gitea, and GitLab.
pub struct LiveAdapters {
    pub forge: Box<dyn Forge>,
    pub backend: ClaudeBackend,
    pub workspace: GitWorkspace,
}

/// Build the adapters. `repo` is the forge-specific target (owner/name for GitHub and
/// Gitea, a project id or URL-encoded path for GitLab); `login` is the `tea` login,
/// required only for Gitea.
pub fn build(
    cfg: &Config,
    kind: ForgeKind,
    login: Option<&str>,
    repo: &str,
    repo_root: PathBuf,
) -> Result<LiveAdapters> {
    let status_labels = cfg.status_labels();
    let forge: Box<dyn Forge> = match kind {
        ForgeKind::GitHub => Box::new(GitHubForge {
            repo: repo.to_string(),
            status_labels,
            repo_root: repo_root.clone(),
        }),
        ForgeKind::Gitea => {
            let login = login.ok_or_else(|| {
                EngineError::Forge(
                    "forge kind 'gitea' requires a login (set [forge].login or --login)".into(),
                )
            })?;
            Box::new(GiteaForge {
                repo: repo.to_string(),
                login: login.to_string(),
                status_labels,
                repo_root: repo_root.clone(),
            })
        }
        ForgeKind::GitLab => Box::new(GitLabForge {
            project: repo.to_string(),
            status_labels,
            repo_root: repo_root.clone(),
        }),
    };
    Ok(LiveAdapters {
        forge,
        backend: ClaudeBackend::default(),
        workspace: GitWorkspace::new(repo_root),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_core::config::{default_roles, ForgeConfig};
    use tutti_core::domain::SelectFilter;
    use tutti_core::gate::Gate;

    fn cfg() -> Config {
        Config {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "version/v0.1".into(),
            model: "m".into(),
            max_issues_per_run: 5,
            select: SelectFilter {
                require_label: "status:ready".into(),
                skip_labels: vec![],
                milestone: None,
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
            roles: default_roles(),
            ci_max_polls: 40,
            poll_delay_secs: 0,
            merge_mode: tutti_core::domain::MergeMode::Merge,
            status: Default::default(),
            forge: ForgeConfig::default(),
        }
    }

    #[test]
    fn builds_each_forge_kind() {
        // GitHub and GitLab need no login; both build.
        assert!(build(&cfg(), ForgeKind::GitHub, None, "o/r", ".".into()).is_ok());
        assert!(build(&cfg(), ForgeKind::GitLab, None, "123", ".".into()).is_ok());
        // Gitea builds when a login is supplied.
        assert!(build(&cfg(), ForgeKind::Gitea, Some("me"), "o/r", ".".into()).is_ok());
    }

    #[test]
    fn gitea_without_login_is_rejected() {
        let err = build(&cfg(), ForgeKind::Gitea, None, "o/r", ".".into());
        assert!(err.is_err());
    }
}
