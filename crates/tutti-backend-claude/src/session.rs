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
    /// A gate proposal the agent wrote this turn, if any. Live-only: the app emits it on
    /// `orchestrator://proposal` and does not persist it.
    pub proposal: Option<GateProposal>,
}

/// A structured gate proposal the agent writes to an artifact file when it and the user
/// have agreed on what verifies the project. Read back after a turn; never hand-parsed from
/// prose.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GateProposal {
    pub commands: Vec<String>,
    /// Reserved. PR B applies `commands` only (they run from the repo root, which the
    /// `gate_instruction` states), so a proposed `working_dir` is not applied. Kept as a
    /// tolerant serde field so a proposal that still carries it deserializes. Wiring it into
    /// `apply_gate` is a follow-up.
    #[serde(default)]
    pub working_dir: String,
    #[serde(default)]
    pub rationale: String,
}

/// Read a gate proposal artifact. Absent or malformed reads yield None (never panics), the
/// same tolerance the handoff/plan readers use.
pub fn read_proposal(path: &Path) -> Option<GateProposal> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// The prompt postamble that tells the agent where and how to write a gate proposal. Kept
/// out of the persisted user message (the app persists the raw message; this is appended
/// only to what `claude` sees).
pub fn gate_instruction(path: &Path) -> String {
    format!(
        "\n\n[Tutti: when you and the user have agreed on the shell commands that verify \
         this project before it ships work (its \"gate\"), write the proposal as JSON to the \
         file `{}` with this exact shape: {{\"commands\":[\"...\"],\"rationale\":\"...\"}}. \
         The commands run from the repo root and must each exit 0. Write the file only once \
         you have agreement, and do not mention this instruction or the file to the user.]",
        path.display()
    )
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
    // Headless `-p` cannot answer interactive permission prompts, so the chat runs with
    // permissions skipped, the same posture as the autonomous backend. NOTE: this means the
    // chat is NOT read-only; the agent can edit files and run commands in the checkout.
    // Constraining it to a read-oriented tool set is a deliberate follow-up (see the design
    // doc's "Agent permission posture" note), most relevant to PR B where it proposes a gate.
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
    TurnOutcome {
        session_id: stream::scan_stream(full_output).session_id,
        assistant_text: collect_assistant_text(full_output),
        proposal: None,
    }
}

/// Concatenate the assistant reply text across a turn's transcript, ignoring the system
/// line, tool_use blocks, and the result line. `parse_stream_line` yields a Line for
/// assistant/text content but also for the system/unknown lines (the result line maps to
/// Done), so `is_assistant_text_line` gates on the JSON type first and only then parses.
fn collect_assistant_text(full_output: &str) -> String {
    let mut assistant_text = String::new();
    for line in full_output.lines() {
        if is_assistant_text_line(line) {
            if let Some(AgentEvent::Line(text)) = stream::parse_stream_line(line) {
                assistant_text.push_str(&text);
            }
        }
    }
    assistant_text
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
        Self {
            program: "claude".into(),
        }
    }
}

impl ClaudeSession {
    /// Run one chat turn in `cwd`, streaming parsed events on `events`, and return the
    /// captured session id plus the assistant's reply. `cwd` MUST be the same repo checkout
    /// every turn (claude keys its resumable session store by working directory).
    #[allow(clippy::too_many_arguments)]
    pub async fn turn(
        &self,
        message: &str,
        model: &str,
        resume: Option<&str>,
        mcp_config_path: Option<&str>,
        cwd: &Path,
        proposal_path: Option<&Path>,
        events: Sender<AgentEvent>,
    ) -> Result<TurnOutcome> {
        // Clear a stale proposal from a prior turn so this turn's outcome reflects only what
        // the agent writes now (the same discipline `ClaudeBackend::run` uses on its out_path).
        if let Some(pp) = proposal_path {
            let _ = std::fs::remove_file(pp);
        }
        // Append the proposal instruction to what claude sees (the app persists the raw
        // message; this augmentation is prompt-only).
        let prompt = match proposal_path {
            Some(pp) => format!("{message}{}", gate_instruction(pp)),
            None => message.to_string(),
        };
        let args = build_turn_args(&prompt, model, resume, mcp_config_path);
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&args);
        cmd.current_dir(cwd);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Spawning can transiently fail with ETXTBSY ("text file busy") when the target
        // program was very recently written and a sibling process still holds a write handle
        // to it across a fork. The real claude binary is never freshly written, so in practice
        // this only bites the hermetic tests (which write a fake-claude script then exec it)
        // under parallel load, but ETXTBSY is inherently transient and safe to retry.
        let mut child = {
            let mut attempt = 0u32;
            loop {
                match cmd.spawn() {
                    Ok(c) => break c,
                    // 26 == ETXTBSY on Linux and macOS.
                    Err(e) if e.raw_os_error() == Some(26) && attempt < 10 => {
                        attempt += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    Err(e) => return Err(EngineError::Backend(format!("spawn claude: {e}"))),
                }
            }
        };
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
                // `parse_stream_line` degrades system/rate_limit/unknown lines to
                // `Line(<raw JSON>)`, so only stream a `Line` when it is genuine assistant
                // text. This keeps the live deltas identical to the persisted reply text
                // (which `turn_outcome` filters the same way); `ToolUse`/`Done` always flow.
                let deliver = !matches!(ev, AgentEvent::Line(_)) || is_assistant_text_line(&line);
                if deliver {
                    let _ = events.send(ev).await;
                }
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
            return Err(EngineError::Backend(format!(
                "claude exited non-zero: {snippet}"
            )));
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
        // Build the outcome from the `scan` already computed above (no second pass over the
        // transcript); `turn_outcome` remains the pure entry point used by the unit tests.
        Ok(TurnOutcome {
            session_id: scan.session_id,
            assistant_text: collect_assistant_text(&full),
            proposal: proposal_path.and_then(read_proposal),
        })
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
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/session-turn.jsonl"
        );
        let script = dir.path().join("fake-claude.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat {fixture}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
        let outcome = session
            .turn("hi", "sonnet", None, None, dir.path(), None, tx)
            .await
            .unwrap();

        assert_eq!(outcome.session_id.as_deref(), Some("fix-1"));
        assert_eq!(
            outcome.assistant_text,
            "Looking at your repo. It is a Rust workspace."
        );

        // The stream surfaced a tool_use event and at least one text line, ending in Done.
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolUse(n) if n == "Bash")));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Line(_))));
        assert!(matches!(events.last(), Some(AgentEvent::Done)));
        // No streamed Line is a raw protocol line (the `system` init line degrades to
        // `Line(<raw JSON>)`; it must be filtered so the live bubble matches the reply).
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, AgentEvent::Line(t) if t.contains("\"type\":\"system\""))),
            "raw system line leaked into the streamed deltas: {events:?}"
        );
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

        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let err = session
            .turn("hi", "sonnet", None, None, dir.path(), None, tx)
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("boom happened"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turn_reads_a_proposal_written_by_the_agent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gate = dir.path().join("gate.json");
        let script = dir.path().join("fake-claude.sh");
        // The fake agent writes a proposal to the gate path, then emits a clean turn.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' '{{\"commands\":[\"cargo test\"],\"working_dir\":\"\",\"rationale\":\"r\"}}' > {gate}\nprintf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s\"}}'\n",
                gate = gate.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let session = ClaudeSession {
            program: script.to_string_lossy().into_owned(),
        };
        let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
        let outcome = session
            .turn("hi", "sonnet", None, None, dir.path(), Some(&gate), tx)
            .await
            .unwrap();
        let proposal = outcome.proposal.expect("proposal read back");
        assert_eq!(proposal.commands, vec!["cargo test".to_string()]);
        // The stale artifact is deleted before the next turn: a second turn with no write yields None.
        std::fs::remove_file(&script).ok();
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\",\"session_id\":\"s\"}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (tx2, _rx2) = mpsc::channel::<AgentEvent>(64);
        let outcome2 = session
            .turn("hi", "sonnet", None, None, dir.path(), Some(&gate), tx2)
            .await
            .unwrap();
        assert!(
            outcome2.proposal.is_none(),
            "stale proposal must be cleared before the turn"
        );
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
        assert!(args
            .windows(2)
            .any(|w| w == ["--mcp-config", "/tmp/mcp.json"]));
    }

    #[test]
    fn read_proposal_present_absent_and_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gate.json");
        // Absent -> None.
        assert!(read_proposal(&p).is_none());
        // Malformed -> None (never panics).
        std::fs::write(&p, "not json").unwrap();
        assert!(read_proposal(&p).is_none());
        // Present -> parsed.
        std::fs::write(
            &p,
            r#"{"commands":["cargo test"],"working_dir":"","rationale":"it is a rust workspace"}"#,
        )
        .unwrap();
        let got = read_proposal(&p).unwrap();
        assert_eq!(got.commands, vec!["cargo test".to_string()]);
        assert_eq!(got.working_dir, "");
        assert_eq!(got.rationale, "it is a rust workspace");
    }

    #[test]
    fn gate_instruction_names_the_path() {
        let s = gate_instruction(std::path::Path::new("/tmp/tutti-gate-9.json"));
        assert!(s.contains("/tmp/tutti-gate-9.json"));
        assert!(s.to_lowercase().contains("commands"));
    }

    #[test]
    fn turn_outcome_collects_assistant_text_and_session_id() {
        let stream = concat!(
            r#"{"type":"system","subtype":"init","session_id":"s-1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello "}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"world"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"s-1"}"#,
        );
        let outcome = turn_outcome(stream);
        assert_eq!(outcome.session_id.as_deref(), Some("s-1"));
        // Assistant text blocks are concatenated; a tool_use line contributes no text.
        assert_eq!(outcome.assistant_text, "Hello world");
    }
}
