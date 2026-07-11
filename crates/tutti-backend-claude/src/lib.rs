// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Claude `AgentBackend`: drives `claude -p` and reads the file-based handoff.

pub mod artifact;
pub mod prompt;
pub mod stream;

use async_trait::async_trait;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;
use tutti_core::message::{AgentEvent, AgentOutcome, AgentStatus, AgentTask, Role, Usage};
use tutti_core::traits::{AgentBackend, EngineError, Result};

/// Drives `claude -p` as a headless coding backend.
pub struct ClaudeBackend {
    /// The `claude` executable (usually just "claude").
    pub program: String,
    /// Extra flags appended to every invocation (permissions, output format).
    pub extra_args: Vec<String>,
}

impl Default for ClaudeBackend {
    fn default() -> Self {
        Self {
            program: "claude".into(),
            extra_args: vec![
                "--output-format".into(),
                "stream-json".into(),
                "--dangerously-skip-permissions".into(),
            ],
        }
    }
}

impl ClaudeBackend {
    /// Build the `AgentOutcome` from a finished run: read the artifact, or map a
    /// missing artifact / usage limit to the right non-panicking status.
    fn outcome_from(
        &self,
        task: &AgentTask,
        out_path: &Path,
        full_output: &str,
    ) -> Result<AgentOutcome> {
        if stream::hit_usage_limit(full_output) {
            return Ok(AgentOutcome {
                status: AgentStatus::Error,
                handoff: None,
                review: None,
                summary: "usage limit reached".into(),
                usage: Usage::default(),
                blocked_reason: Some("usage limit reached".into()),
            });
        }
        if task.playbook.role == Role::Reviewer {
            let review = artifact::read_review(out_path)?;
            return Ok(AgentOutcome {
                status: if review.is_some() {
                    AgentStatus::ReadyToShip
                } else {
                    AgentStatus::Blocked
                },
                handoff: None,
                review,
                summary: "review complete".into(),
                usage: Usage::default(),
                blocked_reason: None,
            });
        }
        let handoff = artifact::read_handoff(out_path)?;
        Ok(AgentOutcome {
            status: if handoff.is_some() {
                AgentStatus::ReadyToShip
            } else {
                AgentStatus::Blocked
            },
            handoff,
            review: None,
            summary: "run complete".into(),
            usage: Usage::default(),
            blocked_reason: if out_path.exists() {
                None
            } else {
                Some("agent produced no handoff".into())
            },
        })
    }
}

#[async_trait]
impl AgentBackend for ClaudeBackend {
    async fn run(
        &self,
        task: AgentTask,
        worktree: &Path,
        events: Sender<AgentEvent>,
    ) -> Result<AgentOutcome> {
        let out_path = prompt::output_path(worktree, task.playbook.role);
        if let Some(parent) = out_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
            let _ = tokio::fs::remove_file(&out_path).await; // stale from a prior attempt
        }
        let prompt = prompt::build_prompt(&task, &out_path);

        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.arg("-p").arg(&prompt).arg("--model").arg(&task.model);
        cmd.args(&self.extra_args);
        cmd.current_dir(worktree);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| EngineError::Backend(format!("spawn claude: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Backend("no stdout".into()))?;
        let mut reader = BufReader::new(stdout).lines();
        let mut full = String::new();
        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| EngineError::Backend(format!("read: {e}")))?
        {
            full.push_str(&line);
            full.push('\n');
            if let Some(ev) = stream::parse_stream_line(&line) {
                let _ = events.send(ev).await;
            }
        }
        let _ = child.wait().await;
        let _ = events.send(AgentEvent::Done).await;

        self.outcome_from(&task, &out_path, &full)
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;
    use tutti_core::domain::{Issue, IssueId};
    use tutti_core::message::RolePlaybook;

    fn task(role: Role) -> AgentTask {
        AgentTask {
            playbook: RolePlaybook {
                role,
                skills: vec![],
            },
            issue: Issue {
                id: IssueId(1),
                title: "t".into(),
                body: "b".into(),
                labels: vec![],
                milestone: None,
            },
            worktree_branch: "feat/issue-1".into(),
            model: "m".into(),
            review: None,
        }
    }

    #[test]
    fn missing_handoff_maps_to_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("handoff.json"),
                "ok",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Blocked);
    }

    #[test]
    fn present_handoff_maps_to_ready() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("handoff.json");
        std::fs::write(&p, r#"{"issue":1,"branch":"feat/issue-1","target":{"target":"version/v0.1","create_from":"main"},"pr_title":"t","pr_body":"b","labels":[],"decision_note":null}"#).unwrap();
        let be = ClaudeBackend::default();
        let out = be.outcome_from(&task(Role::Implementer), &p, "ok").unwrap();
        assert_eq!(out.status, AgentStatus::ReadyToShip);
        assert!(out.handoff.is_some());
    }

    #[test]
    fn usage_limit_maps_to_error() {
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("h.json"),
                "hit the usage limit",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Error);
    }
}
