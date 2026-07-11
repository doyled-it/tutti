// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Claude `AgentBackend`: drives `claude -p` and reads the file-based handoff.

pub mod artifact;
pub mod prompt;
pub mod stream;

use async_trait::async_trait;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
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
        exit_success: bool,
        stderr: &str,
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
            if review.is_some() {
                return Ok(AgentOutcome {
                    status: AgentStatus::ReadyToShip,
                    handoff: None,
                    review,
                    summary: "review complete".into(),
                    usage: Usage::default(),
                    blocked_reason: None,
                });
            }
            return Ok(no_artifact_outcome(exit_success, stderr, "review complete"));
        }
        let handoff = artifact::read_handoff(out_path)?;
        if handoff.is_some() {
            return Ok(AgentOutcome {
                status: AgentStatus::ReadyToShip,
                handoff,
                review: None,
                summary: "run complete".into(),
                usage: Usage::default(),
                blocked_reason: None,
            });
        }
        Ok(no_artifact_outcome(exit_success, stderr, "run complete"))
    }
}

/// Build the outcome when no handoff/review artifact was produced. A non-zero exit means
/// the process actually failed (crash, bad flags), which is an `Error` carrying a short
/// stderr snippet, not a `Blocked` (which is the agent cleanly finishing without a
/// handoff). A clean zero exit with no artifact is a genuine block.
fn no_artifact_outcome(exit_success: bool, stderr: &str, summary: &str) -> AgentOutcome {
    if exit_success {
        return AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            summary: summary.into(),
            usage: Usage::default(),
            blocked_reason: Some("agent produced no handoff".into()),
        };
    }
    let snippet: String = stderr.trim().chars().take(500).collect();
    let reason = format!("claude exited non-zero: {snippet}");
    AgentOutcome {
        status: AgentStatus::Error,
        handoff: None,
        review: None,
        summary: reason.clone(),
        usage: Usage::default(),
        blocked_reason: Some(reason),
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
        // Drain stderr concurrently. A long run that fills the ~64KB stderr pipe would
        // otherwise deadlock: the child blocks on the full pipe while we block reading
        // stdout, and neither side advances.
        let stderr_pipe = child.stderr.take();
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            if let Some(pipe) = stderr_pipe {
                let _ = BufReader::new(pipe).read_to_string(&mut buf).await;
            }
            buf
        });

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
        let status = child
            .wait()
            .await
            .map_err(|e| EngineError::Backend(format!("wait: {e}")))?;
        let stderr = stderr_task.await.unwrap_or_default();
        let _ = events.send(AgentEvent::Done).await;

        self.outcome_from(&task, &out_path, &full, status.success(), &stderr)
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
                true,
                "",
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
        let out = be
            .outcome_from(&task(Role::Implementer), &p, "ok", true, "")
            .unwrap();
        assert_eq!(out.status, AgentStatus::ReadyToShip);
        assert!(out.handoff.is_some());
    }

    #[test]
    fn usage_limit_maps_to_error() {
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        // The limit marker must appear on a structured result-type line to trip detection.
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("h.json"),
                r#"{"type":"result","subtype":"error","result":"Claude usage limit reached"}"#,
                true,
                "",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Error);
    }

    #[test]
    fn nonzero_exit_without_artifact_maps_to_error() {
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("handoff.json"),
                "some output",
                false,
                "fatal: boom happened",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Error);
        assert!(out.blocked_reason.as_deref().unwrap().contains("boom"));
    }
}
