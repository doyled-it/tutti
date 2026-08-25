// SPDX-License-Identifier: AGPL-3.0-or-later
//! Evaluation records and a deterministic proxy scorer for a skill, plus a runner over a
//! transcript-source seam.
//!
//! This is the hermetic half of skill evaluation: loading eval records, a substring-based
//! proxy score, and a runner driven by a `SkillTranscriptSource` (a fake in tests). The
//! live behavioral eval, which drives a real backend and judges the transcript against
//! `expected_behavior` semantically (a model as judge, not a substring match), is
//! deliberately out of scope here and belongs to a later live-tier increment.

use crate::error::DesignError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One evaluation scenario for a skill (Anthropic's eval-record shape). `expected_behavior`
/// is a rubric of observable outcomes the skill's output should exhibit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalRecord {
    pub skills: Vec<String>,
    pub query: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    pub expected_behavior: Vec<String>,
}

/// Load a skill directory's eval records from `<dir>/evals.json` (a JSON array).
/// `Ok(vec![])` when the file is absent.
pub fn load_evals(skill_dir: &Path) -> Result<Vec<EvalRecord>, DesignError> {
    let path = skill_dir.join("evals.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| DesignError::Skill(format!("reading {}: {e}", path.display())))?;
    let records: Vec<EvalRecord> = serde_json::from_str(&content)?;
    Ok(records)
}

/// True when a skill carries the minimum number of evals Anthropic's evaluation-driven
/// authoring calls for (>= 3).
pub fn has_minimum_evals(records: &[EvalRecord]) -> bool {
    records.len() >= 3
}

/// The outcome of scoring one record against a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOutcome {
    /// One entry per `expected_behavior`, true when that outcome was observed.
    pub met: Vec<bool>,
    /// True when every expected behavior was met.
    pub passed: bool,
}

/// Deterministic proxy scoring: each `expected_behavior` is "met" when it appears as a
/// case-insensitive substring of the transcript. (A semantic rubric judged by a model is
/// the live-tier eval; this deterministic form is what the hermetic harness runs.)
pub fn score(record: &EvalRecord, transcript: &str) -> EvalOutcome {
    let haystack = transcript.to_lowercase();
    let met: Vec<bool> = record
        .expected_behavior
        .iter()
        .map(|behavior| haystack.contains(&behavior.to_lowercase()))
        .collect();
    let passed = met.iter().all(|&m| m);
    EvalOutcome { met, passed }
}

/// A source of the transcript a skill produces for an eval record. The live implementation
/// drives a real backend; the hermetic tests use a fake.
pub trait SkillTranscriptSource {
    fn transcript_for(&self, record: &EvalRecord) -> Result<String, DesignError>;
}

/// Run every record through the source and score it. Returns one outcome per record.
pub fn run_evals(
    records: &[EvalRecord],
    source: &dyn SkillTranscriptSource,
) -> Result<Vec<EvalOutcome>, DesignError> {
    records
        .iter()
        .map(|record| {
            let transcript = source.transcript_for(record)?;
            Ok(score(record, &transcript))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> EvalRecord {
        EvalRecord {
            skills: vec!["pdf-extractor".to_string()],
            query: "Extract the text from report.pdf".to_string(),
            inputs: vec!["report.pdf".to_string()],
            expected_behavior: vec![
                "Filtered the rows".to_string(),
                "Reported the row count".to_string(),
            ],
        }
    }

    #[test]
    fn eval_record_round_trips_json() {
        let json = serde_json::json!({
            "skills": ["pdf-extractor"],
            "query": "Extract the text",
            "expected_behavior": ["Extracted the text"]
        });
        let record: EvalRecord = serde_json::from_value(json).unwrap();
        assert_eq!(record.inputs, Vec::<String>::new());

        let round_tripped: EvalRecord =
            serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(round_tripped, record);
    }

    #[test]
    fn load_evals_absent_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_evals(dir.path()).unwrap(), Vec::new());
    }

    #[test]
    fn load_evals_reads_a_json_array() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![record(), record(), record()];
        std::fs::write(
            dir.path().join("evals.json"),
            serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        let loaded = load_evals(dir.path()).unwrap();
        assert_eq!(loaded.len(), 3);
    }

    #[test]
    fn has_minimum_evals_requires_three() {
        assert!(!has_minimum_evals(&[record(), record()]));
        assert!(has_minimum_evals(&[record(), record(), record()]));
    }

    #[test]
    fn score_all_met_passes() {
        let r = record();
        let transcript = "First I filtered the rows, then I reported the row count.";
        let outcome = score(&r, transcript);
        assert!(outcome.passed);
        assert_eq!(outcome.met, vec![true, true]);
    }

    #[test]
    fn score_is_case_insensitive() {
        let r = record();
        let transcript = "the pipeline FILTERED THE ROWS and reported the row count.";
        let outcome = score(&r, transcript);
        assert!(outcome.passed);
    }

    #[test]
    fn score_one_missing_fails() {
        let r = record();
        let transcript = "First I filtered the rows.";
        let outcome = score(&r, transcript);
        assert!(!outcome.passed);
        assert_eq!(outcome.met, vec![true, false]);
    }

    struct FakeSource;

    impl SkillTranscriptSource for FakeSource {
        fn transcript_for(&self, record: &EvalRecord) -> Result<String, DesignError> {
            if record.query.contains("Extract") {
                Ok("Filtered the rows and reported the row count.".to_string())
            } else {
                Ok("Did nothing useful.".to_string())
            }
        }
    }

    #[test]
    fn run_evals_scores_each_record_via_the_source() {
        let matching = record();
        let mut non_matching = record();
        non_matching.query = "Do something else entirely".to_string();

        let records = vec![matching, non_matching];
        let outcomes = run_evals(&records, &FakeSource).unwrap();

        assert_eq!(outcomes.len(), 2);
        assert!(outcomes[0].passed);
        assert!(!outcomes[1].passed);
    }
}
