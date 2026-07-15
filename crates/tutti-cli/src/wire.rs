// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build the concrete engine (GitHub forge + Claude backend + git workspace) from config.

use std::path::PathBuf;
use tutti_backend_claude::ClaudeBackend;
use tutti_core::config::Config;
use tutti_forge_github::GitHubForge;
use tutti_git::GitWorkspace;

/// The concrete adapter set the CLI runs with.
pub struct LiveAdapters {
    pub forge: GitHubForge,
    pub backend: ClaudeBackend,
    pub workspace: GitWorkspace,
}

/// Build the adapters from config + the repo slug + repo root on disk.
pub fn build(cfg: &Config, repo: &str, repo_root: PathBuf) -> LiveAdapters {
    LiveAdapters {
        forge: GitHubForge {
            repo: repo.to_string(),
            status_labels: cfg.status.clone(),
            repo_root: repo_root.clone(),
        },
        backend: ClaudeBackend::default(),
        workspace: GitWorkspace::new(repo_root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_core::config::default_roles;
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
        }
    }

    #[test]
    fn build_uses_config_labels_and_repo() {
        let a = build(&cfg(), "o/r", PathBuf::from("."));
        assert_eq!(a.forge.repo, "o/r");
        assert_eq!(a.forge.status_labels.ready, "status:ready");
        assert_eq!(a.forge.repo_root, PathBuf::from("."));
    }
}
