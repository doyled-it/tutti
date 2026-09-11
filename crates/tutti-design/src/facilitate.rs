// SPDX-License-Identifier: AGPL-3.0-or-later
//! The single-movement facilitation loop: a consumer-owned conversation seam, the tagged
//! agent reply and its parser, and the step function that drives one movement to
//! ratification.

use crate::error::{DesignError, Result};
use serde::Deserialize;

/// One raw turn from the conversation backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTurn {
    /// The agent conversation id to resume on the next turn.
    pub session_id: String,
    /// The agent's raw reply text (a tagged JSON object, possibly wrapped in prose).
    pub output: String,
}

/// The conversation seam: one facilitation turn. `resume` is the prior turn's `session_id`
/// (None on the first turn). Consumer-owned so E2 stays hermetic; the real adapter over
/// `ClaudeSession` is wired at the CLI/Tauri surface (E7).
#[allow(async_fn_in_trait)]
pub trait Facilitator {
    async fn turn(&self, prompt: &str, resume: Option<&str>) -> Result<RawTurn>;
}

/// The agent's structured reply for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MovementReply {
    /// The agent needs more from the human before it can cover the movement.
    Ask { question: String },
    /// The agent judges the movement covered and proposes its artifact section.
    Complete {
        artifact_section: String,
        diagrams: Vec<String>,
    },
}

// Internal serde shapes for parsing the tagged object.
#[derive(Deserialize)]
struct CompleteBody {
    artifact_section: String,
    #[serde(default)]
    diagrams: Vec<String>,
}
#[derive(Deserialize)]
struct ReplyWire {
    ask: Option<String>,
    complete: Option<CompleteBody>,
}

/// Parse a tagged reply from the agent's raw output. Extracts the first balanced JSON object
/// (the reply may be wrapped in prose or a fenced code block), then requires exactly one of
/// `ask` / `complete`. A reply carrying neither is an error, never a silent pass.
pub fn parse_reply(output: &str) -> Result<MovementReply> {
    let json = extract_json_object(output)
        .ok_or_else(|| DesignError::Facilitation("no JSON object in the agent reply".into()))?;
    let wire: ReplyWire = serde_json::from_str(json)
        .map_err(|e| DesignError::Facilitation(format!("reply is not valid reply JSON: {e}")))?;
    match (wire.ask, wire.complete) {
        (Some(question), None) => Ok(MovementReply::Ask { question }),
        (None, Some(body)) => Ok(MovementReply::Complete {
            artifact_section: body.artifact_section,
            diagrams: body.diagrams,
        }),
        (Some(_), Some(_)) => Err(DesignError::Facilitation(
            "reply carried both `ask` and `complete`".into(),
        )),
        (None, None) => Err(DesignError::Facilitation(
            "reply carried neither `ask` nor `complete`".into(),
        )),
    }
}

/// Find the first balanced `{...}` object in `s`, respecting string literals and escapes, so
/// a brace inside a JSON string does not throw off the balance. Returns the slice or None.
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&s[start..=i]);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_ask_reply() {
        let r = parse_reply(r#"{"ask": "Who is this for?"}"#).unwrap();
        assert_eq!(
            r,
            MovementReply::Ask {
                question: "Who is this for?".into()
            }
        );
    }

    #[test]
    fn parses_a_complete_reply_with_diagrams() {
        let r = parse_reply(
            r###"{"complete": {"artifact_section": "## Constitution\nprivacy first", "diagrams": ["flow"]}}"###,
        )
        .unwrap();
        match r {
            MovementReply::Complete {
                artifact_section,
                diagrams,
            } => {
                assert!(artifact_section.contains("privacy first"));
                assert_eq!(diagrams, vec!["flow".to_string()]);
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn tolerates_prose_around_the_json_object() {
        // A `claude -p` reply may wrap the JSON in prose or a fenced block; extract the object.
        let r = parse_reply("Sure!\n```json\n{\"ask\": \"What is out of scope?\"}\n```\nhope that helps")
            .unwrap();
        assert_eq!(
            r,
            MovementReply::Ask {
                question: "What is out of scope?".into()
            }
        );
    }

    #[test]
    fn a_reply_with_neither_tag_is_an_error_not_a_silent_pass() {
        assert!(parse_reply(r#"{"nope": 1}"#).is_err());
        assert!(parse_reply("no json here at all").is_err());
    }

    #[test]
    fn complete_defaults_diagrams_to_empty_when_omitted() {
        let r = parse_reply(r#"{"complete": {"artifact_section": "x"}}"#).unwrap();
        match r {
            MovementReply::Complete { diagrams, .. } => assert!(diagrams.is_empty()),
            _ => panic!("expected Complete"),
        }
    }
}
