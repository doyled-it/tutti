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
}
