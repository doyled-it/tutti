// SPDX-License-Identifier: AGPL-3.0-or-later
//! The gate: a config-declared command list that must all exit 0.

use crate::traits::{EngineError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// A gate is data, not a trait: commands run in `working_dir` relative to the repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub commands: Vec<String>,
    #[serde(default)]
    pub working_dir: PathBuf,
}

/// The result of running the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOutcome {
    pub passed: bool,
    pub log: String,
}

impl Gate {
    /// Run every command in sequence under `repo`/`working_dir`. Stops at the first failure.
    pub async fn run(&self, repo: &Path) -> Result<GateOutcome> {
        let cwd = repo.join(&self.working_dir);
        let mut log = String::new();
        for cmd in &self.commands {
            log.push_str(&format!("$ {cmd}\n"));
            let output = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(&cwd)
                .output()
                .await
                .map_err(|e| EngineError::Gate(format!("spawn `{cmd}`: {e}")))?;
            log.push_str(&String::from_utf8_lossy(&output.stdout));
            log.push_str(&String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                return Ok(GateOutcome { passed: false, log });
            }
        }
        Ok(GateOutcome { passed: true, log })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn passes_when_all_commands_exit_zero() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Gate {
            commands: vec!["true".into(), "echo hi".into()],
            working_dir: PathBuf::new(),
        };
        let out = gate.run(dir.path()).await.unwrap();
        assert!(out.passed, "log: {}", out.log);
    }

    #[tokio::test]
    async fn fails_and_stops_at_first_nonzero() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Gate {
            commands: vec!["false".into(), "echo should-not-run".into()],
            working_dir: PathBuf::new(),
        };
        let out = gate.run(dir.path()).await.unwrap();
        assert!(!out.passed);
        assert!(!out.log.contains("should-not-run"));
    }
}
