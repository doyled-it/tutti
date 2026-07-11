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

use crate::stream::ResultEvent;

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
                // `claude -p --output-format stream-json` refuses to launch without
                // `--verbose`; it errors out with "requires --verbose". This flag is not
                // optional for the live path.
                "--verbose".into(),
                "--dangerously-skip-permissions".into(),
            ],
        }
    }
}

impl ClaudeBackend {
    /// Build the `AgentOutcome` from a finished run. Outcome and error detection are driven
    /// off the structured signals `claude` emits (the authoritative `result` event and any
    /// `rate_limit_event`), not substring scanning, with `hit_usage_limit` kept only as a
    /// defensive fallback when no `result` event was seen at all.
    ///
    /// Precedence: (1) a detected usage/rate limit -> `Error`; (2) a `result` event that
    /// reports `is_error` -> `Error`; (3) a present handoff/review artifact -> `ReadyToShip`;
    /// (4) a non-zero exit with no artifact -> `Error` with a stderr snippet; (5) otherwise a
    /// clean finish with no artifact -> `Blocked`.
    #[allow(clippy::too_many_arguments)]
    fn outcome_from(
        &self,
        task: &AgentTask,
        out_path: &Path,
        result: Option<&ResultEvent>,
        rate_limited: bool,
        full_output: &str,
        exit_success: bool,
        stderr: &str,
    ) -> Result<AgentOutcome> {
        // Prefer the token accounting the `result` event carries, falling back to zero.
        let usage = result
            .and_then(|r| r.usage)
            .map(|(input_tokens, output_tokens)| Usage {
                input_tokens,
                output_tokens,
            })
            .unwrap_or_default();

        // (1) Usage/rate limit. Keyed off the structured `rate_limit_event` status and the
        // result event's `api_error_status`; the substring scan is only a last resort when no
        // result event was captured.
        let api_status_is_limit = result
            .and_then(|r| r.api_error_status.as_deref())
            .map(|s| {
                let l = s.to_lowercase();
                l.contains("rate") || l.contains("limit")
            })
            .unwrap_or(false);
        let limit = rate_limited
            || api_status_is_limit
            || (result.is_none() && stream::hit_usage_limit(full_output));
        if limit {
            return Ok(AgentOutcome {
                status: AgentStatus::Error,
                handoff: None,
                review: None,
                summary: "usage/rate limit".into(),
                usage,
                blocked_reason: Some("usage/rate limit".into()),
            });
        }

        // (2) The result event explicitly reports an error. This outranks a present artifact:
        // a failed run should not be shipped even if a stale handoff is on disk.
        if let Some(r) = result {
            if r.is_error {
                let mut reason = String::from("claude reported an error");
                if let Some(status) = &r.api_error_status {
                    reason.push_str(&format!(" ({status})"));
                }
                if !r.result.is_empty() {
                    let snippet: String = r.result.trim().chars().take(500).collect();
                    reason.push_str(&format!(": {snippet}"));
                }
                return Ok(AgentOutcome {
                    status: AgentStatus::Error,
                    handoff: None,
                    review: None,
                    summary: reason.clone(),
                    usage,
                    blocked_reason: Some(reason),
                });
            }
        }

        // (3) A present handoff/review artifact is the ship signal.
        if task.playbook.role == Role::Reviewer {
            let review = artifact::read_review(out_path)?;
            if review.is_some() {
                return Ok(AgentOutcome {
                    status: AgentStatus::ReadyToShip,
                    handoff: None,
                    review,
                    summary: "review complete".into(),
                    usage,
                    blocked_reason: None,
                });
            }
            return Ok(no_artifact_outcome(
                exit_success,
                stderr,
                "review complete",
                usage,
            ));
        }
        let handoff = artifact::read_handoff(out_path)?;
        if handoff.is_some() {
            return Ok(AgentOutcome {
                status: AgentStatus::ReadyToShip,
                handoff,
                review: None,
                summary: "run complete".into(),
                usage,
                blocked_reason: None,
            });
        }
        // (4)/(5) No artifact: non-zero exit is an Error, a clean exit is a genuine Block.
        Ok(no_artifact_outcome(
            exit_success,
            stderr,
            "run complete",
            usage,
        ))
    }
}

/// Build the outcome when no handoff/review artifact was produced. A non-zero exit means
/// the process actually failed (crash, bad flags), which is an `Error` carrying a short
/// stderr snippet, not a `Blocked` (which is the agent cleanly finishing without a
/// handoff). A clean zero exit with no artifact is a genuine block.
fn no_artifact_outcome(
    exit_success: bool,
    stderr: &str,
    summary: &str,
    usage: Usage,
) -> AgentOutcome {
    if exit_success {
        return AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            summary: summary.into(),
            usage,
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
        usage,
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

        // Pull the authoritative structured signals out of the transcript: the last `result`
        // event and whether any `rate_limit_event` reported a non-"allowed" status.
        let scan = stream::scan_stream(&full);
        self.outcome_from(
            &task,
            &out_path,
            scan.result.as_ref(),
            scan.rate_limited,
            &full,
            status.success(),
            &stderr,
        )
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

    /// The captured real transcript, compiled in so the outcome tests are hermetic.
    const REAL_STREAM: &str = include_str!("../tests/fixtures/real-stream.jsonl");

    #[test]
    fn verbose_is_in_default_args() {
        // `claude -p --output-format stream-json` refuses to launch without `--verbose`.
        assert!(ClaudeBackend::default()
            .extra_args
            .contains(&"--verbose".to_string()));
    }

    #[test]
    fn missing_handoff_maps_to_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("handoff.json"),
                None,
                false,
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
            .outcome_from(&task(Role::Implementer), &p, None, false, "ok", true, "")
            .unwrap();
        assert_eq!(out.status, AgentStatus::ReadyToShip);
        assert!(out.handoff.is_some());
    }

    #[test]
    fn real_result_plus_handoff_maps_to_ready_and_carries_usage() {
        // The real success `result` event (is_error false) plus a present handoff ships, and
        // the token accounting from the result event is threaded into the outcome.
        let scan = stream::scan_stream(REAL_STREAM);
        assert!(scan.result.is_some());
        assert!(!scan.rate_limited);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("handoff.json");
        std::fs::write(&p, r#"{"issue":1,"branch":"feat/issue-1","target":{"target":"version/v0.1","create_from":"main"},"pr_title":"t","pr_body":"b","labels":[],"decision_note":null}"#).unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &p,
                scan.result.as_ref(),
                scan.rate_limited,
                REAL_STREAM,
                true,
                "",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::ReadyToShip);
        assert_eq!(out.usage.input_tokens, 2);
        assert_eq!(out.usage.output_tokens, 4);
    }

    #[test]
    fn result_is_error_without_artifact_maps_to_error() {
        let synthetic = ResultEvent {
            is_error: true,
            subtype: "error".into(),
            api_error_status: None,
            result: "boom happened".into(),
            usage: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("handoff.json"),
                Some(&synthetic),
                false,
                "",
                true,
                "",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Error);
        assert!(out.blocked_reason.as_deref().unwrap().contains("boom"));
    }

    #[test]
    fn rate_limit_event_rejected_maps_to_error() {
        // A structured rate_limit_event whose status is not "allowed" is a limit.
        let scan = stream::scan_stream(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#,
        );
        assert!(scan.rate_limited);
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("handoff.json"),
                scan.result.as_ref(),
                scan.rate_limited,
                "",
                true,
                "",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Error);
        assert!(out.summary.contains("limit"));
    }

    #[test]
    fn usage_limit_maps_to_error() {
        let dir = tempfile::tempdir().unwrap();
        let be = ClaudeBackend::default();
        // Defensive fallback: no result event captured, but the transcript carries a clear
        // limit phrase on a structured result-type line.
        let out = be
            .outcome_from(
                &task(Role::Implementer),
                &dir.path().join("h.json"),
                None,
                false,
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
                None,
                false,
                "some output",
                false,
                "fatal: boom happened",
            )
            .unwrap();
        assert_eq!(out.status, AgentStatus::Error);
        assert!(out.blocked_reason.as_deref().unwrap().contains("boom"));
    }
}
