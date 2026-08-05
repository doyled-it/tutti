// SPDX-License-Identifier: AGPL-3.0-or-later
//! Parse `claude --output-format stream-json` lines into engine `AgentEvent`s.
//! Unknown or unparseable lines degrade to `AgentEvent::Line` so a CLI change cannot
//! break the adapter.

use serde_json::Value;
use tutti_core::message::AgentEvent;

/// Parse one stream-json line into an event fit to show a human, or None when the line
/// carries no displayable content.
///
/// `parse_stream_events` deliberately degrades system/rate_limit/unknown lines to
/// `Line(<raw JSON>)` so a CLI change can never break the adapter, but that raw JSON must
/// never reach a transcript: it would render as assistant prose. This gates on
/// `is_assistant_text_line` so only genuine assistant text streams as a `Line`;
/// `ToolUse`/`Done` always flow.
///
/// Both live streaming paths (`ClaudeSession::turn` for the orchestrator chat and
/// `ClaudeBackend::run` for the engine's per-role subsessions) go through this one
/// function, so their filtering cannot drift.
pub fn parse_display_events(line: &str) -> Vec<AgentEvent> {
    let assistant = is_assistant_text_line(line);
    parse_stream_events(line)
        .into_iter()
        .filter(|ev| !matches!(ev, AgentEvent::Line(_)) || assistant)
        .collect()
}

/// True when a stream-json line is an assistant/text message (not system/result/unknown),
/// so only genuine reply text is streamed or accumulated.
pub fn is_assistant_text_line(line: &str) -> bool {
    serde_json::from_str::<Value>(line.trim())
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "assistant" || t == "text")
        })
        .unwrap_or(false)
}

/// Parse one stream-json line into zero or more events, in order.
///
/// A line yields more than one event when a single assistant message carries several
/// content blocks (text then tool_use is the common real shape); it yields none only for a
/// blank line, or an assistant message whose content array holds nothing usable.
pub fn parse_stream_events(line: &str) -> Vec<AgentEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        return vec![AgentEvent::Line(trimmed.to_string())];
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") | Some("text") => {
            // Real `claude` stream-json nests assistant output under `message.content`,
            // which is an ARRAY of typed blocks ({"type":"text",...} / {"type":"tool_use",...}).
            // Handle that first, then the flat-string and top-level-`text` shapes.
            // Shapes confirmed defensively; verify against a captured real transcript when
            // the live tier runs.
            let from_blocks = v
                .get("message")
                .and_then(|m| m.get("content"))
                .map(events_from_content)
                .unwrap_or_default();
            if !from_blocks.is_empty() {
                return from_blocks;
            }
            let text = v
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            vec![AgentEvent::Line(text)]
        }
        Some("tool_use") => {
            let name = v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("tool")
                .to_string();
            vec![AgentEvent::ToolUse(name)]
        }
        Some("result") => vec![AgentEvent::Done],
        _ => vec![AgentEvent::Line(trimmed.to_string())],
    }
}

/// Turn a `message.content` value into events, in block order. Accepts the flat-string
/// shape and the real array-of-blocks shape.
///
/// **One message can carry several blocks, and every one of them must come out.** Real
/// `claude --output-format stream-json` routinely emits
/// `content: [{"type":"text",...},{"type":"tool_use",...}]` — a sentence of preamble
/// followed by the tool call it introduces. The original version returned a single event
/// with text winning, so every tool call announced that way vanished from the Subsessions
/// pane, and multiple tool calls in one message lost all but the first. The committed
/// fixture happened to keep text and tool_use in separate messages, so nothing caught it.
///
/// Consecutive text blocks are still merged into one `Line`, which is the behaviour
/// `collect_assistant_text` relies on for the persisted transcript; a `tool_use` flushes
/// whatever text is pending first, so ordering within the message is preserved.
fn events_from_content(content: &serde_json::Value) -> Vec<AgentEvent> {
    if let Some(s) = content.as_str() {
        return vec![AgentEvent::Line(s.to_string())];
    }
    let Some(arr) = content.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut text = String::new();
    for block in arr {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
            Some("tool_use") => {
                if !text.is_empty() {
                    out.push(AgentEvent::Line(std::mem::take(&mut text)));
                }
                out.push(AgentEvent::ToolUse(
                    block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("tool")
                        .to_string(),
                ));
            }
            _ => {}
        }
    }
    if !text.is_empty() {
        out.push(AgentEvent::Line(text));
    }
    out
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
    /// claude's session id, captured from any line that carries a top-level `session_id`
    /// (the `system` init line and the `result` line both do). Used to `--resume` the chat.
    pub session_id: Option<String>,
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
        if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
            scan.session_id = Some(sid.to_string()); // last one wins; they are identical
        }
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

    /// The single event a line yields, for the many cases where exactly one is expected.
    /// None when the line yields nothing; panics if a test points it at a multi-block line,
    /// which is what `events_from_content` tests use directly.
    fn one_event(line: &str) -> Option<AgentEvent> {
        let evs = parse_stream_events(line);
        assert!(evs.len() <= 1, "expected at most one event, got {evs:?}");
        evs.into_iter().next()
    }

    fn one_display(line: &str) -> Option<AgentEvent> {
        let evs = parse_display_events(line);
        assert!(evs.len() <= 1, "expected at most one event, got {evs:?}");
        evs.into_iter().next()
    }

    #[test]
    fn tool_use_maps_to_tooluse_event() {
        let e = one_event(r#"{"type":"tool_use","name":"Edit"}"#).unwrap();
        assert_eq!(e, AgentEvent::ToolUse("Edit".into()));
    }

    #[test]
    fn result_maps_to_done() {
        assert_eq!(one_event(r#"{"type":"result"}"#).unwrap(), AgentEvent::Done);
    }

    #[test]
    fn unknown_json_degrades_to_line() {
        assert!(matches!(
            one_event(r#"{"weird":1}"#).unwrap(),
            AgentEvent::Line(_)
        ));
    }

    #[test]
    fn non_json_degrades_to_line() {
        assert_eq!(
            one_event("plain text").unwrap(),
            AgentEvent::Line("plain text".into())
        );
    }

    #[test]
    fn blank_line_is_none() {
        assert!(one_event("   ").is_none());
    }

    #[test]
    fn content_array_text_blocks_concatenate_to_line() {
        let e = one_event(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello "},{"type":"text","text":"world"}]}}"#,
        )
        .unwrap();
        assert_eq!(e, AgentEvent::Line("Hello world".into()));
    }

    #[test]
    fn content_array_tool_use_block_maps_to_tooluse() {
        let e = one_event(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        )
        .unwrap();
        assert_eq!(e, AgentEvent::ToolUse("Bash".into()));
    }

    #[test]
    fn flat_string_content_still_maps_to_line() {
        let e = one_event(r#"{"type":"assistant","message":{"content":"flat"}}"#).unwrap();
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

    #[test]
    fn scan_captures_session_id_from_result_and_system_lines() {
        // system init line carries session_id; result line carries it too. Either populates it.
        let stream = concat!(
            r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"hi","session_id":"abc-123"}"#,
        );
        let scan = scan_stream(stream);
        assert_eq!(scan.session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn scan_session_id_none_when_absent() {
        let scan = scan_stream(r#"{"type":"assistant","message":{"content":"hi"}}"#);
        assert_eq!(scan.session_id, None);
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
            one_event(real_line(1)).unwrap(),
            AgentEvent::Line("hello".into())
        );
    }

    #[test]
    fn real_tool_use_line_maps_to_tooluse() {
        // Line 2: assistant message with a tool_use block naming "Edit".
        assert_eq!(
            one_event(real_line(2)).unwrap(),
            AgentEvent::ToolUse("Edit".into())
        );
    }

    #[test]
    fn real_result_line_maps_to_done() {
        // Line 5: the terminal result event.
        assert_eq!(one_event(real_line(5)).unwrap(), AgentEvent::Done);
    }

    #[test]
    fn a_message_carrying_both_text_and_a_tool_use_yields_both_in_order() {
        // Line 4 of the captured transcript: the shape real `claude` emits constantly, a
        // sentence of preamble followed by the tool call it introduces. The parser used to
        // return ONE event per line with text winning, so every tool call announced this way
        // vanished from the Subsessions pane. The fixture previously kept the two block types
        // in separate messages, which is exactly why nothing caught it.
        assert_eq!(
            parse_stream_events(real_line(4)),
            vec![
                AgentEvent::Line("Let me check the config.".into()),
                AgentEvent::ToolUse("Read".into()),
            ]
        );
        // And both survive the display filter, since the line IS assistant text.
        assert_eq!(parse_display_events(real_line(4)).len(), 2);
    }

    #[test]
    fn several_tool_uses_in_one_message_all_come_through() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"Read"},
            {"type":"tool_use","name":"Edit"},
            {"type":"tool_use","name":"Bash"}
        ]}}"#;
        assert_eq!(
            parse_stream_events(line),
            vec![
                AgentEvent::ToolUse("Read".into()),
                AgentEvent::ToolUse("Edit".into()),
                AgentEvent::ToolUse("Bash".into()),
            ]
        );
    }

    #[test]
    fn text_around_a_tool_use_keeps_its_order() {
        // Trailing commentary after a tool call must land after it, not be merged into the
        // preamble. Consecutive text blocks still merge, which is what
        // `collect_assistant_text` relies on for the persisted transcript.
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"before "},
            {"type":"text","text":"the call"},
            {"type":"tool_use","name":"Read"},
            {"type":"text","text":"after"}
        ]}}"#;
        assert_eq!(
            parse_stream_events(line),
            vec![
                AgentEvent::Line("before the call".into()),
                AgentEvent::ToolUse("Read".into()),
                AgentEvent::Line("after".into()),
            ]
        );
    }

    #[test]
    fn display_event_drops_the_raw_protocol_lines_of_a_real_transcript() {
        // The captured transcript's system-init (0) and rate_limit_event (3) lines degrade to
        // `Line(<raw JSON>)` under `parse_stream_events`. They must never reach a transcript:
        // `parse_display_events` drops them, while text/tool_use/result still flow.
        assert_eq!(one_display(real_line(0)), None);
        assert_eq!(one_display(real_line(3)), None);
        assert_eq!(
            one_display(real_line(1)),
            Some(AgentEvent::Line("hello".into()))
        );
        assert_eq!(
            one_display(real_line(2)),
            Some(AgentEvent::ToolUse("Edit".into()))
        );
        assert_eq!(one_display(real_line(5)), Some(AgentEvent::Done));
    }

    #[test]
    fn display_event_drops_unparseable_and_unknown_lines() {
        // A non-JSON line and an unknown `type` both degrade to a raw `Line`; neither is
        // assistant text, so neither is displayable.
        assert_eq!(one_display("not json at all"), None);
        assert_eq!(one_display(r#"{"type":"user","message":{}}"#), None);
        assert_eq!(one_display("   "), None);
    }

    #[test]
    fn is_assistant_text_line_gates_on_the_json_type() {
        assert!(is_assistant_text_line(
            r#"{"type":"assistant","message":{"content":"hi"}}"#
        ));
        assert!(is_assistant_text_line(r#"{"type":"text","text":"hi"}"#));
        assert!(!is_assistant_text_line(r#"{"type":"system"}"#));
        assert!(!is_assistant_text_line("garbage"));
    }

    #[test]
    fn real_system_and_rate_limit_lines_degrade_to_line() {
        // system init (line 0) and rate_limit_event (line 3) are not modelled as events; they
        // degrade to Line so a CLI change can never break the adapter.
        assert!(matches!(
            one_event(real_line(0)).unwrap(),
            AgentEvent::Line(_)
        ));
        assert!(matches!(
            one_event(real_line(3)).unwrap(),
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
