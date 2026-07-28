// SPDX-License-Identifier: AGPL-3.0-or-later
//! The orchestrator chat session: drives `claude -p` as a resumable, streaming multi-turn
//! conversation in the repo checkout. Reuses `stream.rs` for parsing and mirrors the
//! spawn/stderr-drain plumbing of `ClaudeBackend::run`, but its outcome is streamed
//! assistant text plus a captured session id, not a ship artifact.

use crate::stream;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::mpsc::Sender;
use tutti_core::message::AgentEvent;
use tutti_core::traits::{EngineError, Result};

/// What one chat turn produced: the (possibly newly captured) session id to resume next
/// time, and the assistant's full reply text for persistence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnOutcome {
    pub session_id: Option<String>,
    pub assistant_text: String,
}

/// Build the `claude -p` argument vector for one chat turn. Pure: no IO. `resume` is the
/// session id to continue (None on the first turn). `mcp_config_path` is an already-written
/// `--mcp-config` file (None when no MCP servers are wired). The message is passed via `-p`.
pub fn build_turn_args(
    message: &str,
    model: &str,
    resume: Option<&str>,
    mcp_config_path: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into(), message.into()];
    args.push("--model".into());
    args.push(model.into());
    args.push("--output-format".into());
    args.push("stream-json".into());
    // `claude -p --output-format stream-json` refuses to launch without --verbose.
    args.push("--verbose".into());
    args.push("--dangerously-skip-permissions".into());
    if let Some(id) = resume {
        args.push("--resume".into());
        args.push(id.into());
    }
    if let Some(cfg) = mcp_config_path {
        // Non-strict (default): a user's own global MCP servers still load alongside.
        args.push("--mcp-config".into());
        args.push(cfg.into());
    }
    args
}

/// Distil a finished turn's full stdout into a `TurnOutcome`: the captured session id and
/// the assistant's reply text. Reuses the `stream.rs` parser so text-block concatenation
/// and tool_use handling stay in one place. Pure: no IO.
pub fn turn_outcome(full_output: &str) -> TurnOutcome {
    let scan = stream::scan_stream(full_output);
    let mut assistant_text = String::new();
    for line in full_output.lines() {
        if let Some(AgentEvent::Line(text)) = stream::parse_stream_line(line) {
            // `parse_stream_line` yields a Line for assistant/text content, for unknown
            // lines, and for the system line (the result line maps to Done instead), so
            // restrict to lines the parser recognized as assistant content by re-checking
            // the type.
            if is_assistant_text_line(line) {
                assistant_text.push_str(&text);
            }
        }
    }
    TurnOutcome { session_id: scan.session_id, assistant_text }
}

/// True when a stream-json line is an assistant/text message (not system/result/unknown),
/// so only genuine reply text is accumulated.
fn is_assistant_text_line(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line.trim())
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "assistant" || t == "text")
        })
        .unwrap_or(false)
}

/// Drives `claude -p` as an interactive chat session. `program` is the executable (usually
/// "claude"; a test points it at a fake script).
pub struct ClaudeSession {
    pub program: String,
}

impl Default for ClaudeSession {
    fn default() -> Self {
        Self { program: "claude".into() }
    }
}

impl ClaudeSession {
    /// Run one chat turn in `cwd`, streaming parsed events on `events`, and return the
    /// captured session id plus the assistant's reply. `cwd` MUST be the same repo checkout
    /// every turn (claude keys its resumable session store by working directory).
    pub async fn turn(
        &self,
        message: &str,
        model: &str,
        resume: Option<&str>,
        mcp_config_path: Option<&str>,
        cwd: &Path,
        events: Sender<AgentEvent>,
    ) -> Result<TurnOutcome> {
        let args = build_turn_args(message, model, resume, mcp_config_path);
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&args);
        cmd.current_dir(cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| EngineError::Backend(format!("spawn claude: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| EngineError::Backend("no stdout".into()))?;
        // Drain stderr concurrently so a full ~64KB pipe cannot deadlock the run.
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

        let scan = stream::scan_stream(&full);
        if !status.success() {
            let snippet: String = stderr.trim().chars().take(500).collect();
            return Err(EngineError::Backend(format!("claude exited non-zero: {snippet}")));
        }
        if scan.rate_limited {
            return Err(EngineError::Backend("usage/rate limit".into()));
        }
        if let Some(r) = &scan.result {
            if r.is_error {
                let mut reason = String::from("claude reported an error");
                if let Some(status) = &r.api_error_status {
                    reason.push_str(&format!(" ({status})"));
                }
                if !r.result.is_empty() {
                    let snippet: String = r.result.trim().chars().take(500).collect();
                    reason.push_str(&format!(": {snippet}"));
                }
                return Err(EngineError::Backend(reason));
            }
        }
        Ok(turn_outcome(&full))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// The `program` a `ClaudeSession` runs. A test points it at a shell script that cats a
    /// fixture to stdout, exercising the spawn + stream + parse loop hermetically.
    #[cfg(unix)]
    #[tokio::test]
    async fn turn_streams_events_and_returns_outcome() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/session-turn.jsonl");
        let script = dir.path().join("fake-claude.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat {fixture}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession { program: script.to_string_lossy().into_owned() };
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
        let outcome = session
            .turn("hi", "sonnet", None, None, dir.path(), tx)
            .await
            .unwrap();

        assert_eq!(outcome.session_id.as_deref(), Some("fix-1"));
        assert_eq!(outcome.assistant_text, "Looking at your repo. It is a Rust workspace.");

        // The stream surfaced a tool_use event and at least one text line, ending in Done.
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolUse(n) if n == "Bash")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Line(_))));
        assert!(matches!(events.last(), Some(AgentEvent::Done)));
    }

    /// `claude -p` can exit 0 while the result line is flagged `is_error`; `turn` must still
    /// fail rather than return a garbage/empty outcome.
    #[cfg(unix)]
    #[tokio::test]
    async fn turn_errors_on_result_flagged_is_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-claude.sh");
        // Exits 0 but the result line is flagged is_error: turn must still fail.
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"boom happened\",\"session_id\":\"s\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession { program: script.to_string_lossy().into_owned() };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let err = session
            .turn("hi", "sonnet", None, None, dir.path(), tx)
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("boom happened"));
    }

    #[test]
    fn first_turn_has_no_resume() {
        let args = build_turn_args("hello", "sonnet", None, None);
        assert!(args.windows(2).any(|w| w == ["-p", "hello"]));
        assert!(args.windows(2).any(|w| w == ["--model", "sonnet"]));
        assert!(args.contains(&"--verbose".to_string()));
        assert!(!args.iter().any(|a| a == "--resume"));
        assert!(!args.iter().any(|a| a == "--mcp-config"));
    }

    #[test]
    fn later_turn_resumes_and_wires_mcp() {
        let args = build_turn_args("next", "sonnet", Some("abc-123"), Some("/tmp/mcp.json"));
        assert!(args.windows(2).any(|w| w == ["--resume", "abc-123"]));
        assert!(args.windows(2).any(|w| w == ["--mcp-config", "/tmp/mcp.json"]));
    }

    #[test]
    fn turn_outcome_collects_assistant_text_and_session_id() {
        let stream = concat!(
            r#"{"type":"system","subtype":"init","session_id":"s-1"}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello "}]}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"world"}]}}"#, "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#, "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s-1"}"#,
        );
        let outcome = turn_outcome(stream);
        assert_eq!(outcome.session_id.as_deref(), Some("s-1"));
        // Assistant text blocks are concatenated; a tool_use line contributes no text.
        assert_eq!(outcome.assistant_text, "Hello world");
    }
}
