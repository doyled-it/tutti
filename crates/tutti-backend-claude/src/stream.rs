// SPDX-License-Identifier: AGPL-3.0-or-later
//! Parse `claude --output-format stream-json` lines into engine `AgentEvent`s.
//! Unknown or unparseable lines degrade to `AgentEvent::Line` so a CLI change cannot
//! break the adapter.

use serde_json::Value;
use tutti_core::message::AgentEvent;

/// Parse one stream-json line. Returns None for blank lines only.
pub fn parse_stream_line(line: &str) -> Option<AgentEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        return Some(AgentEvent::Line(trimmed.to_string()));
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") | Some("text") => {
            let text = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .or_else(|| v.get("text").and_then(|t| t.as_str()))
                .unwrap_or("")
                .to_string();
            Some(AgentEvent::Line(text))
        }
        Some("tool_use") => {
            let name = v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("tool")
                .to_string();
            Some(AgentEvent::ToolUse(name))
        }
        Some("result") => Some(AgentEvent::Done),
        _ => Some(AgentEvent::Line(trimmed.to_string())),
    }
}

/// Scan a run's full output for a usage/rate-limit marker.
pub fn hit_usage_limit(full_output: &str) -> bool {
    let lower = full_output.to_lowercase();
    ["usage limit", "rate limit", "session limit"]
        .iter()
        .any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_use_maps_to_tooluse_event() {
        let e = parse_stream_line(r#"{"type":"tool_use","name":"Edit"}"#).unwrap();
        assert_eq!(e, AgentEvent::ToolUse("Edit".into()));
    }

    #[test]
    fn result_maps_to_done() {
        assert_eq!(
            parse_stream_line(r#"{"type":"result"}"#).unwrap(),
            AgentEvent::Done
        );
    }

    #[test]
    fn unknown_json_degrades_to_line() {
        assert!(matches!(
            parse_stream_line(r#"{"weird":1}"#).unwrap(),
            AgentEvent::Line(_)
        ));
    }

    #[test]
    fn non_json_degrades_to_line() {
        assert_eq!(
            parse_stream_line("plain text").unwrap(),
            AgentEvent::Line("plain text".into())
        );
    }

    #[test]
    fn blank_line_is_none() {
        assert!(parse_stream_line("   ").is_none());
    }

    #[test]
    fn usage_limit_detected() {
        assert!(hit_usage_limit("... Claude usage limit reached ..."));
        assert!(!hit_usage_limit("all good"));
    }
}
