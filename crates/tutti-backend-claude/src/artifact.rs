// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read the file-based result an agent writes (`.tutti/handoff.json` / `review.json`).

use std::path::Path;
use tutti_core::message::{Handoff, PlanDecision, ReviewReport};
use tutti_core::traits::{EngineError, Result};

/// Read and parse a `Handoff` from `path`. `Ok(None)` if the file is absent (the agent
/// finished without emitting a handoff); `Err` if present but malformed.
pub fn read_handoff(path: &Path) -> Result<Option<Handoff>> {
    read_json(path)
}

/// Read and parse a `ReviewReport`. `Ok(None)` if absent.
pub fn read_review(path: &Path) -> Result<Option<ReviewReport>> {
    read_json(path)
}

/// Read and parse a `PlanDecision` (the Planner's `.tutti/plan.json`). `Ok(None)` if absent.
pub fn read_plan(path: &Path) -> Result<Option<PlanDecision>> {
    read_json(path)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(EngineError::Backend(format!(
                "read {}: {e}",
                path.display()
            )))
        }
    };
    let parsed = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(first) => {
            // The agent occasionally writes a string value with an invalid backslash escape: a
            // lone `\` or a `\x` sequence from prose like a regex, a path, or math notation,
            // which JSON rejects. Repair invalid escapes (turn them into literal backslashes)
            // and retry; a structural error that the repair cannot fix still fails with the
            // ORIGINAL parse message, so this never masks a genuinely broken file.
            let repaired = repair_invalid_escapes(&text);
            serde_json::from_str(&repaired)
                .map_err(|_| EngineError::Backend(format!("parse {}: {first}", path.display())))?
        }
    };
    Ok(Some(parsed))
}

/// Repair invalid JSON backslash escapes by turning each one into a literal backslash
/// (`\x` -> `\\x`). In JSON a backslash is only legal before one of `" \ / b f n r t u`; any
/// other backslash (or a trailing one) is invalid, which is the agent's most common handoff
/// mistake when a string value carries prose with a stray backslash. Valid escapes (including
/// `\\` and `\uXXXX`) are preserved untouched.
///
/// Known limitation (fails safe): a string value ending in a literal backslash right before its
/// closing quote (`"...\"`) reads as a valid `\"` escape and is not recovered. The retry then
/// fails too, so the run halts with the original parse error rather than a silent misparse.
fn repair_invalid_escapes(s: &str) -> String {
    const VALID: &[char] = &['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // A valid escape: emit the pair verbatim and consume the escape char, so an escaped
            // backslash (`\\`) is never re-examined and double-counted.
            Some(&next) if VALID.contains(&next) => {
                out.push('\\');
                out.push(next);
                chars.next();
            }
            // An invalid or trailing backslash: escape it so the string parses as a literal `\`.
            _ => out.push_str("\\\\"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_core::message::PlanAction;

    #[test]
    fn absent_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_handoff(&dir.path().join("nope.json"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn valid_handoff_parses() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("handoff.json");
        std::fs::write(&p, r#"{"issue":5,"branch":"feat/issue-5","target":{"target":"version/v0.1","create_from":"main"},"pr_title":"t","pr_body":"b","labels":[],"decision_note":null}"#).unwrap();
        let h = read_handoff(&p).unwrap().unwrap();
        assert_eq!(h.issue.0, 5);
        assert_eq!(h.target.target, "version/v0.1");
    }

    #[test]
    fn valid_plan_parses() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("plan.json");
        std::fs::write(
            &p,
            r#"{"action":{"CreateIssues":[{"title":"x","body":"","labels":[]}]},"rationale":"r","needs_human":false}"#,
        )
        .unwrap();
        let plan = read_plan(&p).unwrap().unwrap();
        assert!(!plan.needs_human);
        // A planner that omits the placement hints still parses, and its issues land at
        // the top level rather than failing the whole decision.
        let PlanAction::CreateIssues(list) = plan.action else {
            panic!("expected CreateIssues");
        };
        assert_eq!(list[0].milestone, None);
        assert_eq!(list[0].epic, None);
    }

    #[test]
    fn plan_carries_placement_hints_when_the_planner_sets_them() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("plan.json");
        std::fs::write(
            &p,
            r#"{"action":{"CreateIssues":[{"title":"x","body":"","labels":[],"milestone":"v0.1","epic":"Tracking rails"}]},"rationale":"r","needs_human":false}"#,
        )
        .unwrap();
        let PlanAction::CreateIssues(list) = read_plan(&p).unwrap().unwrap().action else {
            panic!("expected CreateIssues");
        };
        assert_eq!(list[0].milestone.as_deref(), Some("v0.1"));
        assert_eq!(list[0].epic.as_deref(), Some("Tracking rails"));
    }

    #[test]
    fn malformed_handoff_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.json");
        std::fs::write(&p, "{not json").unwrap();
        assert!(read_handoff(&p).is_err());
    }

    #[test]
    fn handoff_with_an_invalid_escape_is_repaired_and_parsed() {
        // The agent wrote a decision_note with a stray backslash (here `\d` from a regex), which
        // JSON rejects as an invalid escape. The reader repairs it rather than halting the run.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("handoff.json");
        std::fs::write(&p, r#"{"issue":5,"branch":"feat/issue-5","target":{"target":"version/v0.1","create_from":"main"},"pr_title":"t","pr_body":"b","labels":[],"decision_note":"match \d+ runs"}"#).unwrap();
        let h = read_handoff(&p).unwrap().unwrap();
        assert_eq!(h.issue.0, 5);
        // The repaired note keeps the literal backslash the agent intended.
        assert_eq!(h.decision_note.as_deref(), Some(r"match \d+ runs"));
    }

    #[test]
    fn repair_preserves_valid_escapes_and_fixes_invalid_ones() {
        // Valid escapes untouched; an escaped backslash stays one; an invalid `\d` is doubled.
        assert_eq!(
            repair_invalid_escapes(r#"{"a":"line\nbreak"}"#),
            r#"{"a":"line\nbreak"}"#
        );
        assert_eq!(
            repair_invalid_escapes(r#"{"a":"C:\\path"}"#),
            r#"{"a":"C:\\path"}"#
        );
        assert_eq!(repair_invalid_escapes(r#"{"a":"\d+"}"#), r#"{"a":"\\d+"}"#);
        // A trailing backslash is escaped, not dropped.
        assert_eq!(repair_invalid_escapes(r"end\"), r"end\\");
    }

    #[test]
    fn repair_does_not_rescue_a_structurally_broken_file() {
        // The repair only fixes escapes; a genuinely broken file still errors (with the
        // original parse message), so a real failure is never masked.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("broken.json");
        std::fs::write(&p, "{not json at all").unwrap();
        assert!(read_handoff(&p).is_err());
    }
}
