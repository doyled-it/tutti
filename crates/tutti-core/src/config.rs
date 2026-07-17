// SPDX-License-Identifier: AGPL-3.0-or-later
//! Project configuration loaded from `tutti.toml`.

use crate::domain::SelectFilter;
use crate::gate::Gate;
use crate::message::Role;
use crate::status::StatusLabels;
use crate::traits::{EngineError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Which forge adapter the CLI drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind {
    #[default]
    GitHub,
    Gitea,
    GitLab,
}

impl std::str::FromStr for ForgeKind {
    type Err = EngineError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "github" => Ok(ForgeKind::GitHub),
            "gitea" => Ok(ForgeKind::Gitea),
            "gitlab" => Ok(ForgeKind::GitLab),
            other => Err(EngineError::Forge(format!(
                "unknown forge kind '{other}' (expected github, gitea, or gitlab)"
            ))),
        }
    }
}

/// Forge selection and connection config (the `[forge]` section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ForgeConfig {
    #[serde(default)]
    pub kind: ForgeKind,
    /// The `tea` login for Gitea/Codeberg (encodes the host). Required for `kind = gitea`;
    /// ignored for GitHub (ambient `gh` auth) and GitLab (ambient `glab` token).
    #[serde(default)]
    pub login: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The protected trunk the engine never merges into.
    pub trunk: String,
    /// Routing strategy name: "trunk" | "phase_stacking".
    pub routing: String,
    /// The integration branch the Trunk strategy targets (ignored by phase_stacking).
    #[serde(default)]
    pub integration_branch: String,
    /// Model tag the backend uses.
    pub model: String,
    #[serde(default = "default_max_issues")]
    pub max_issues_per_run: u32,
    /// How many times the executor polls CI before giving up.
    #[serde(default = "default_ci_max_polls")]
    pub ci_max_polls: u32,
    /// Seconds the executor sleeps between CI polls.
    #[serde(default = "default_poll_delay_secs")]
    pub poll_delay_secs: u64,
    pub select: SelectFilter,
    pub gate: Gate,
    /// Issue status labels the engine flips (ready -> in-progress -> done).
    /// When the `[status]` section is absent, `ready` falls back to
    /// `select.require_label` (so `claim`/`record` strip the very label selection
    /// gates on, as before 3B) and `in_progress`/`done` use the `status:*`
    /// convention. The section is all-or-nothing: if present, all three fields are
    /// required. Read the resolved value via `status_labels()`, not this field.
    #[serde(default)]
    pub status: Option<StatusLabels>,
    /// Which forge to drive and how to authenticate. Defaults to GitHub when absent.
    #[serde(default)]
    pub forge: ForgeConfig,
    /// role -> skill refs. Roles absent here fall back to `default_roles()`.
    #[serde(default)]
    pub roles: HashMap<Role, Vec<String>>,
    /// How the executor merges a shipped PR. Defaults to a merge commit, never squash.
    #[serde(default = "default_merge_mode")]
    pub merge_mode: crate::domain::MergeMode,
}

fn default_max_issues() -> u32 {
    25
}

fn default_ci_max_polls() -> u32 {
    40
}

fn default_poll_delay_secs() -> u64 {
    15
}

fn default_merge_mode() -> crate::domain::MergeMode {
    crate::domain::MergeMode::Merge
}

/// The shipped default role -> skills mapping.
pub fn default_roles() -> HashMap<Role, Vec<String>> {
    HashMap::from([
        (
            Role::Implementer,
            vec![
                "superpowers:subagent-driven-development".into(),
                "superpowers:test-driven-development".into(),
            ],
        ),
        (
            Role::Reviewer,
            vec!["superpowers:requesting-code-review".into()],
        ),
        (
            Role::FixApplier,
            vec!["superpowers:receiving-code-review".into()],
        ),
        (Role::Planner, vec!["tutti:planning".into()]),
    ])
}

impl Config {
    /// Load and validate config from a TOML file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| EngineError::Forge(format!("read {}: {e}", path.display())))?;
        let mut cfg: Config = toml::from_str(&text)
            .map_err(|e| EngineError::Forge(format!("parse {}: {e}", path.display())))?;
        for (role, skills) in default_roles() {
            cfg.roles.entry(role).or_insert(skills);
        }
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.routing == "trunk" && self.integration_branch.is_empty() {
            return Err(EngineError::Routing(
                "routing=trunk requires integration_branch".into(),
            ));
        }
        if self.integration_branch == self.trunk {
            return Err(EngineError::Guardrail(
                "integration_branch must not equal trunk".into(),
            ));
        }
        // Selection gates on `require_label`; `claim`/`record` strip the `ready`
        // status label. If those disagree, a claimed issue never leaves the ready
        // set and the drain loop reprocesses it. Enforce that they match.
        let status = self.status_labels();
        if status.ready != self.select.require_label {
            return Err(EngineError::Guardrail(format!(
                "status.ready ({}) must equal select.require_label ({})",
                status.ready, self.select.require_label
            )));
        }
        Ok(())
    }

    /// The resolved status labels: the `[status]` section when present, otherwise
    /// the shipped convention with `ready` taken from `select.require_label`.
    pub fn status_labels(&self) -> StatusLabels {
        self.status.clone().unwrap_or_else(|| StatusLabels {
            ready: self.select.require_label.clone(),
            ..StatusLabels::default()
        })
    }

    /// The skills for `role`, falling back to the shipped default when the role
    /// is absent from `self.roles` (e.g. a Config built directly, not loaded).
    pub fn skills_for(&self, role: Role) -> Vec<String> {
        self.roles
            .get(&role)
            .cloned()
            .unwrap_or_else(|| default_roles().get(&role).cloned().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_defaults_when_absent_and_overrides_when_present() {
        use crate::status::StatusLabels;

        // Absent: falls back to the shipped default triple.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "staging"
model = "m"

[select]
require_label = "status:ready"
skip_labels = []

[gate]
commands = ["true"]
working_dir = ""
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        // require_label is the default "status:ready", so the resolved triple is
        // exactly the shipped default.
        assert_eq!(cfg.status_labels(), StatusLabels::default());

        // Present: overrides.
        let p2 = dir.path().join("tutti2.toml");
        std::fs::write(
            &p2,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "staging"
model = "m"

[select]
require_label = "todo"
skip_labels = []

[gate]
commands = ["true"]
working_dir = ""

[status]
ready = "todo"
in_progress = "doing"
done = "shipped"
"#,
        )
        .unwrap();
        let cfg2 = Config::load(&p2).unwrap();
        assert_eq!(cfg2.status_labels().ready, "todo");
        assert_eq!(cfg2.status_labels().in_progress, "doing");
        assert_eq!(cfg2.status_labels().done, "shipped");
    }

    #[test]
    fn absent_status_takes_ready_from_require_label() {
        // A custom require_label with no [status] section must still drop claimed
        // issues out of the ready set: the resolved ready label follows require_label.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "staging"
model = "m"

[select]
require_label = "todo"
skip_labels = []

[gate]
commands = ["true"]
working_dir = ""
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.status_labels().ready, "todo");
        assert_eq!(cfg.status_labels().in_progress, "status:in-progress");
    }

    #[test]
    fn status_ready_must_match_require_label() {
        // A [status].ready that disagrees with select.require_label is rejected at
        // load, so the drain loop cannot silently reprocess a claimed issue.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "staging"
model = "m"

[select]
require_label = "status:ready"
skip_labels = []

[gate]
commands = ["true"]
working_dir = ""

[status]
ready = "mismatch"
in_progress = "doing"
done = "shipped"
"#,
        )
        .unwrap();
        assert!(Config::load(&p).is_err());
    }

    #[test]
    fn loads_and_fills_default_roles() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "version/v0.1"
model = "claude-opus-4-8"

[select]
require_label = "status:ready"
skip_labels = ["status:needs-human"]

[gate]
commands = ["cargo test"]
working_dir = ""
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.trunk, "main");
        assert!(cfg
            .skills_for(Role::Reviewer)
            .contains(&"superpowers:requesting-code-review".to_string()));
    }

    #[test]
    fn parses_explicit_roles_table() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "version/v0.1"
model = "claude-opus-4-8"

[select]
require_label = "status:ready"
skip_labels = ["status:needs-human"]

[gate]
commands = ["cargo test"]
working_dir = ""

[roles]
implementer = ["custom:my-implement-skill"]
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(
            cfg.skills_for(Role::Implementer),
            vec!["custom:my-implement-skill".to_string()]
        );
        assert!(cfg
            .skills_for(Role::Reviewer)
            .contains(&"superpowers:requesting-code-review".to_string()));
    }

    #[test]
    fn skills_for_falls_back_to_default_when_role_absent() {
        let cfg = Config {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "version/v0.1".into(),
            model: "m".into(),
            max_issues_per_run: 25,
            ci_max_polls: 40,
            poll_delay_secs: 15,
            select: SelectFilter {
                require_label: "status:ready".into(),
                skip_labels: vec![],
                milestone: None,
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
            status: None,
            forge: Default::default(),
            roles: HashMap::new(),
            merge_mode: crate::domain::MergeMode::Merge,
        };
        assert_eq!(
            cfg.skills_for(Role::Reviewer),
            default_roles().get(&Role::Reviewer).cloned().unwrap()
        );
    }

    #[test]
    fn rejects_integration_branch_equal_to_trunk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "main"
model = "m"
[select]
require_label = "status:ready"
skip_labels = []
[gate]
commands = ["true"]
working_dir = ""
"#,
        )
        .unwrap();
        assert!(Config::load(&p).is_err());
    }

    #[test]
    fn parses_merge_mode_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "version/v0.1"
model = "m"
merge_mode = "squash"
[select]
require_label = "status:ready"
skip_labels = []
[gate]
commands = ["true"]
working_dir = ""
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.merge_mode, crate::domain::MergeMode::Squash);

        let p2 = dir.path().join("tutti-default.toml");
        std::fs::write(
            &p2,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "version/v0.1"
model = "m"
[select]
require_label = "status:ready"
skip_labels = []
[gate]
commands = ["true"]
working_dir = ""
"#,
        )
        .unwrap();
        let cfg2 = Config::load(&p2).unwrap();
        assert_eq!(cfg2.merge_mode, crate::domain::MergeMode::Merge);
    }

    #[test]
    fn forge_defaults_to_github_when_absent() {
        use crate::config::ForgeKind;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "staging"
model = "m"

[select]
require_label = "status:ready"
skip_labels = []

[gate]
commands = ["true"]
working_dir = ""
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.forge.kind, ForgeKind::GitHub);
        assert_eq!(cfg.forge.login, None);
    }

    #[test]
    fn forge_section_parses_kind_and_login() {
        use crate::config::ForgeKind;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tutti.toml");
        std::fs::write(
            &p,
            r#"
trunk = "main"
routing = "trunk"
integration_branch = "staging"
model = "m"

[select]
require_label = "status::ready"
skip_labels = []

[gate]
commands = ["true"]
working_dir = ""

[forge]
kind = "gitea"
login = "icesight-engine"
"#,
        )
        .unwrap();
        let cfg = Config::load(&p).unwrap();
        assert_eq!(cfg.forge.kind, ForgeKind::Gitea);
        assert_eq!(cfg.forge.login.as_deref(), Some("icesight-engine"));
    }

    #[test]
    fn forge_kind_from_str() {
        use crate::config::ForgeKind;
        assert_eq!("github".parse::<ForgeKind>().unwrap(), ForgeKind::GitHub);
        assert_eq!("gitea".parse::<ForgeKind>().unwrap(), ForgeKind::Gitea);
        assert_eq!("gitlab".parse::<ForgeKind>().unwrap(), ForgeKind::GitLab);
        assert!("bogus".parse::<ForgeKind>().is_err());
    }
}
