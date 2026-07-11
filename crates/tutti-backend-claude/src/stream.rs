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
            // Real `claude` stream-json nests assistant output under `message.content`,
            // which is an ARRAY of typed blocks ({"type":"text",...} / {"type":"tool_use",...}).
            // Handle that first, then the flat-string and top-level-`text` shapes.
            // Shapes confirmed defensively; verify against a captured real transcript when
            // the live tier runs.
            if let Some(ev) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(event_from_content)
            {
                return Some(ev);
            }
            let text = v
                .get("text")
                .and_then(|t| t.as_str())
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

/// Turn a `message.content` value into a single event. Accepts the flat-string shape and
/// the real array-of-blocks shape: text blocks are concatenated into a `Line`; a
/// `tool_use` block (when there is no text) becomes a `ToolUse` by name. Returns None when
/// nothing usable is present so the caller can fall through to its other shapes.
fn event_from_content(content: &serde_json::Value) -> Option<AgentEvent> {
    if let Some(s) = content.as_str() {
        return Some(AgentEvent::Line(s.to_string()));
    }
    let arr = content.as_array()?;
    let mut text = String::new();
    let mut tool: Option<String> = None;
    for block in arr {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
            Some("tool_use") if tool.is_none() => {
                tool = Some(
                    block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("tool")
                        .to_string(),
                );
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        Some(AgentEvent::Line(text))
    } else {
        tool.map(AgentEvent::ToolUse)
    }
}

/// Scan a run's full output for a usage/rate-limit marker. Restricted to structured
/// `result`/`system` stream-json lines so that a run whose task or code merely discusses
/// rate limiting in ordinary assistant text is not misclassified as limit-hit.
pub fn hit_usage_limit(full_output: &str) -> bool {
    const MARKERS: [&str; 3] = ["usage limit", "rate limit", "session limit"];
    full_output.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            return false;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("result") | Some("system") => {
                let lower = trimmed.to_lowercase();
                MARKERS.iter().any(|m| lower.contains(m))
            }
            _ => false,
        }
    })
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
    fn content_array_text_blocks_concatenate_to_line() {
        let e = parse_stream_line(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello "},{"type":"text","text":"world"}]}}"#,
        )
        .unwrap();
        assert_eq!(e, AgentEvent::Line("Hello world".into()));
    }

    #[test]
    fn content_array_tool_use_block_maps_to_tooluse() {
        let e = parse_stream_line(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        )
        .unwrap();
        assert_eq!(e, AgentEvent::ToolUse("Bash".into()));
    }

    #[test]
    fn flat_string_content_still_maps_to_line() {
        let e = parse_stream_line(r#"{"type":"assistant","message":{"content":"flat"}}"#).unwrap();
        assert_eq!(e, AgentEvent::Line("flat".into()));
    }

    #[test]
    fn usage_limit_detected_on_result_line() {
        // A limit marker inside a structured result-type line trips detection.
        assert!(hit_usage_limit(
            r#"{"type":"result","subtype":"error","result":"Claude usage limit reached"}"#
        ));
        // System-type lines also count.
        assert!(hit_usage_limit(
            r#"{"type":"system","message":"session limit exceeded"}"#
        ));
    }

    #[test]
    fn usage_limit_ignored_in_assistant_text() {
        // The same phrase in ordinary assistant text must NOT trip: the agent is merely
        // discussing rate limiting, not actually blocked by it.
        assert!(!hit_usage_limit(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"we should handle the rate limit here"}]}}"#
        ));
        assert!(!hit_usage_limit("all good"));
    }
}
