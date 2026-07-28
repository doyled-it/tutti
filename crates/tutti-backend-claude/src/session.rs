// SPDX-License-Identifier: AGPL-3.0-or-later
//! The orchestrator chat session: drives `claude -p` as a resumable, streaming multi-turn
//! conversation in the repo checkout. Reuses `stream.rs` for parsing and mirrors the
//! spawn/stderr-drain plumbing of `ClaudeBackend::run`, but its outcome is streamed
//! assistant text plus a captured session id, not a ship artifact.

#![allow(unused_imports)]

use crate::stream;
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;
use tutti_core::mcp::McpServer;
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
            // `parse_stream_line` only yields a Line for assistant/text content and for
            // unknown lines; the system/result lines degrade to Line too, so restrict to
            // lines the parser recognized as assistant content by re-checking the type.
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
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(|s| s.to_string()))
        .map(|t| t == "assistant" || t == "text")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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
