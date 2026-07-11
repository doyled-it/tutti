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

/// The authoritative `result` event that closes a `stream-json` run. This, not substring
/// scanning, is the source of truth for a run's outcome, error state, and token accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultEvent {
    /// Whether `claude` flagged the run as failed.
    pub is_error: bool,
    /// e.g. "success" or "error".
    pub subtype: String,
    /// Populated by the CLI when an API-level error (including rate limiting) occurred.
    pub api_error_status: Option<String>,
    /// The final result text (may be an error message when `is_error`).
    pub result: String,
    /// `(input_tokens, output_tokens)` when the event carried usage.
    pub usage: Option<(u64, u64)>,
}

/// The structured signals distilled from a whole transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamScan {
    /// The LAST `result` event seen (authoritative outcome), if any.
    pub result: Option<ResultEvent>,
    /// True when any `rate_limit_event` reported a status other than "allowed".
    pub rate_limited: bool,
}

/// Parse one already-decoded stream-json value as a `result` event. Returns None when the
/// value is not a `result` line.
fn parse_result_event(v: &Value) -> Option<ResultEvent> {
    if v.get("type").and_then(|t| t.as_str()) != Some("result") {
        return None;
    }
    let usage = v.get("usage").and_then(|u| {
        let input = u.get("input_tokens").and_then(|n| n.as_u64())?;
        let output = u.get("output_tokens").and_then(|n| n.as_u64())?;
        Some((input, output))
    });
    Some(ResultEvent {
        is_error: v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false),
        subtype: v
            .get("subtype")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        api_error_status: v
            .get("api_error_status")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string()),
        result: v
            .get("result")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string(),
        usage,
    })
}

/// Distil the structured outcome signals from a run's full transcript: the last `result`
/// event and whether any `rate_limit_event` reported a non-"allowed" status. Prefer this over
/// substring scanning to decide a run's fate.
pub fn scan_stream(full_output: &str) -> StreamScan {
    let mut scan = StreamScan::default();
    for line in full_output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("result") => {
                if let Some(r) = parse_result_event(&v) {
                    scan.result = Some(r); // keep the LAST one
                }
            }
            Some("rate_limit_event") => {
                // "allowed" means the request was NOT limited; anything else (rejected,
                // blocked, exhausted, ...) is a real limit.
                let status = v
                    .get("rate_limit_info")
                    .and_then(|i| i.get("status"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("allowed");
                if !status.eq_ignore_ascii_case("allowed") {
                    scan.rate_limited = true;
                }
            }
            _ => {}
        }
    }
    scan
}

/// Scan a run's full output for a usage/rate-limit marker. Restricted to structured
/// `result`/`system` stream-json lines so that a run whose task or code merely discusses
/// rate limiting in ordinary assistant text is not misclassified as limit-hit. This is now
/// only a defensive fallback: `scan_stream` and the structured `result`/`rate_limit_event`
/// are the primary signals.
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

    /// The captured real transcript (5 lines: system init, assistant text, assistant
    /// tool_use, rate_limit_event, result), compiled in so these tests are hermetic.
    const REAL_STREAM: &str = include_str!("../tests/fixtures/real-stream.jsonl");

    fn real_line(idx: usize) -> &'static str {
        REAL_STREAM.lines().nth(idx).expect("fixture line")
    }

    #[test]
    fn real_assistant_text_line_maps_to_line() {
        // Line 1 (0-indexed): assistant message with a text block.
        assert_eq!(
            parse_stream_line(real_line(1)).unwrap(),
            AgentEvent::Line("hello".into())
        );
    }

    #[test]
    fn real_tool_use_line_maps_to_tooluse() {
        // Line 2: assistant message with a tool_use block naming "Edit".
        assert_eq!(
            parse_stream_line(real_line(2)).unwrap(),
            AgentEvent::ToolUse("Edit".into())
        );
    }

    #[test]
    fn real_result_line_maps_to_done() {
        // Line 4: the terminal result event.
        assert_eq!(parse_stream_line(real_line(4)).unwrap(), AgentEvent::Done);
    }

    #[test]
    fn real_system_and_rate_limit_lines_degrade_to_line() {
        // system init (line 0) and rate_limit_event (line 3) are not modelled as events; they
        // degrade to Line so a CLI change can never break the adapter.
        assert!(matches!(
            parse_stream_line(real_line(0)).unwrap(),
            AgentEvent::Line(_)
        ));
        assert!(matches!(
            parse_stream_line(real_line(3)).unwrap(),
            AgentEvent::Line(_)
        ));
    }

    #[test]
    fn scan_of_real_stream_is_success_not_limited() {
        let scan = scan_stream(REAL_STREAM);
        let result = scan.result.expect("result event captured");
        assert!(!result.is_error);
        assert_eq!(result.subtype, "success");
        assert_eq!(result.api_error_status, None);
        assert_eq!(result.result, "hello");
        assert_eq!(result.usage, Some((2, 4)));
        // "allowed" rate_limit_event must NOT count as limited.
        assert!(!scan.rate_limited);
    }

    #[test]
    fn scan_flags_non_allowed_rate_limit_event() {
        let scan =
            scan_stream(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#);
        assert!(scan.rate_limited);
        assert!(scan.result.is_none());
    }

    #[test]
    fn scan_keeps_last_result_event() {
        let stream = concat!(
            r#"{"type":"result","subtype":"error","is_error":true,"result":"first"}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"second"}"#,
        );
        let result = scan_stream(stream).result.unwrap();
        assert_eq!(result.result, "second");
        assert!(!result.is_error);
    }
}
