// SPDX-License-Identifier: AGPL-3.0-or-later
//! Project configuration loaded from `tutti.toml`.

use crate::domain::SelectFilter;
use crate::gate::Gate;
use crate::message::Role;
use crate::traits::{EngineError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

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
        Ok(())
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
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
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
}
