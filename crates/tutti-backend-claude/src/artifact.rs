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
    let parsed = serde_json::from_str(&text)
        .map_err(|e| EngineError::Backend(format!("parse {}: {e}", path.display())))?;
    Ok(Some(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn malformed_handoff_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.json");
        std::fs::write(&p, "{not json").unwrap();
        assert!(read_handoff(&p).is_err());
    }
}
