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
    pub select: SelectFilter,
    pub gate: Gate,
    /// role -> skill refs. Roles absent here fall back to `default_roles()`.
    #[serde(default)]
    pub roles: HashMap<Role, Vec<String>>,
}

fn default_max_issues() -> u32 {
    25
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

    /// The skills for `role`, falling back to the shipped default.
    pub fn skills_for(&self, role: Role) -> Vec<String> {
        self.roles.get(&role).cloned().unwrap_or_default()
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
}
