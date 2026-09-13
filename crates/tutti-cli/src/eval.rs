// SPDX-License-Identifier: AGPL-3.0-or-later
//! `tutti eval`: the live behavioral eval. Drives a real `claude` session over a skill's
//! eval records and judges each transcript against its `expected_behavior` rubric
//! semantically (a model as judge), rather than the hermetic substring proxy.
//!
//! The judging abstraction and the runner live in `tutti-design` (`Judge`,
//! `run_evals_judged`) and are hermetic. This module supplies the two live seams:
//! `ClaudeTranscriptSource` (runs a skill + a record's query through `ClaudeSession`) and
//! `ClaudeJudge` (asks the backend for a per-behavior verdict and parses it), plus the
//! `tutti eval` command that wires them together.
//!
//! Both seams bridge the synchronous `SkillTranscriptSource`/`Judge` traits onto the async
//! `ClaudeSession::turn` by holding a runtime handle and `block_on`-ing the turn. They must
//! therefore be driven from a thread that is not itself a runtime worker (the `tutti eval`
//! command runs `run` on a dedicated thread; the live test builds its own runtime on the
//! test thread), or `block_on` would panic.

use std::path::PathBuf;
use tutti_backend_claude::session::ClaudeSession;
use tutti_core::message::AgentEvent;
use tutti_design::{DesignError, EvalOutcome, EvalRecord, Judge, Skill, SkillTranscriptSource};

/// A live transcript source: runs a skill plus one eval record's query through a real
/// `claude` session and returns the assistant's reply as the transcript to judge.
pub struct ClaudeTranscriptSource {
    pub session: ClaudeSession,
    pub model: String,
    pub skill: Skill,
    /// The working directory for the turn. A throwaway temp dir: the eval is not repo-bound.
    pub cwd: PathBuf,
    pub handle: tokio::runtime::Handle,
}

impl ClaudeTranscriptSource {
    /// Build the prompt for a record: the skill body, then the scenario, then any inputs.
    fn prompt_for(&self, record: &EvalRecord) -> String {
        let mut prompt = self.skill.body.clone();
        prompt.push_str("\n\nScenario: ");
        prompt.push_str(&record.query);
        if !record.inputs.is_empty() {
            prompt.push_str("\n\nInputs:\n");
            prompt.push_str(&record.inputs.join("\n"));
        }
        prompt
    }
}

impl SkillTranscriptSource for ClaudeTranscriptSource {
    fn transcript_for(&self, record: &EvalRecord) -> Result<String, DesignError> {
        let prompt = self.prompt_for(record);
        let outcome = self
            .handle
            .block_on(run_turn(&self.session, &prompt, &self.model, &self.cwd))
            .map_err(|e| DesignError::Facilitation(format!("eval transcript turn: {e}")))?;
        Ok(outcome.assistant_text)
    }
}

/// A live model-as-judge: asks the backend whether the transcript exhibits each expected
/// behavior and parses the returned `{"met": [..]}` verdict.
pub struct ClaudeJudge {
    pub session: ClaudeSession,
    pub model: String,
    /// The working directory for the judging turn. A throwaway temp dir.
    pub cwd: PathBuf,
    pub handle: tokio::runtime::Handle,
}

/// The parsed judge verdict: one boolean per expected behavior, in order.
#[derive(serde::Deserialize)]
struct Verdict {
    met: Vec<bool>,
}

impl Judge for ClaudeJudge {
    fn judge(&self, record: &EvalRecord, transcript: &str) -> Result<EvalOutcome, DesignError> {
        let prompt = judging_prompt(&record.expected_behavior, transcript);
        let outcome = self
            .handle
            .block_on(run_turn(&self.session, &prompt, &self.model, &self.cwd))
            .map_err(|e| DesignError::Facilitation(format!("eval judge turn: {e}")))?;

        // Try each balanced JSON object in the reply (a chatty judge may wrap the verdict in
        // prose or emit an earlier brace group), returning the first that parses as a Verdict
        // of the right length. A wrong-length verdict or an unparseable reply is an error, never
        // a silent pass/fail.
        let want = record.expected_behavior.len();
        let mut last_err =
            "the judge did not return a JSON verdict object of the right length".to_string();
        for object in balanced_objects(&outcome.assistant_text) {
            match serde_json::from_str::<Verdict>(object) {
                Ok(v) if v.met.len() == want => {
                    let passed = v.met.iter().all(|&m| m);
                    return Ok(EvalOutcome { met: v.met, passed });
                }
                Ok(v) => {
                    last_err = format!(
                        "the judge returned {} verdict(s) for {want} expected behavior(s)",
                        v.met.len()
                    );
                }
                Err(e) => last_err = format!("could not parse the judge verdict: {e}"),
            }
        }
        Err(DesignError::Facilitation(last_err))
    }
}

/// Run one `claude` turn to completion, draining the streamed events so the bounded channel
/// never backs the turn up. Returns the turn outcome (its assistant reply is what callers
/// want). Must be awaited from inside a runtime (the seams `block_on` it).
async fn run_turn(
    session: &ClaudeSession,
    prompt: &str,
    model: &str,
    cwd: &std::path::Path,
) -> tutti_core::traits::Result<tutti_backend_claude::session::TurnOutcome> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let outcome = session.turn(prompt, model, None, None, cwd, None, tx).await;
    let _ = drain.await;
    outcome
}

/// Build the judging prompt: number each expected behavior and ask for a single JSON verdict.
fn judging_prompt(expected_behavior: &[String], transcript: &str) -> String {
    let mut behaviors = String::new();
    for (i, behavior) in expected_behavior.iter().enumerate() {
        behaviors.push_str(&format!("{}. {}\n", i + 1, behavior));
    }
    format!(
        "You are grading whether a transcript exhibits specific expected behaviors. For each \
         numbered behavior, answer whether the transcript exhibits it. Reply with a single JSON \
         object and nothing else: {{\"met\": [true/false, ...]}} with one boolean per behavior, \
         in order.\n\n\
         Expected behaviors:\n{behaviors}\n\
         Transcript:\n{transcript}"
    )
}

/// Every top-level balanced `{...}` object in `text`, in order, robust to prose around them and
/// to braces and quotes inside JSON strings. The delimiters scanned (`{`, `}`, `"`, `\\`) are
/// all ASCII, so each returned slice sits on char boundaries. The judge tries each in turn, so a
/// chatty reply with an earlier brace group does not defeat the real verdict object.
fn balanced_objects(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut depth = 0usize;
            let mut in_string = false;
            let mut escaped = false;
            let mut end = None;
            for (offset, &byte) in bytes.iter().enumerate().skip(i) {
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                    continue;
                }
                match byte {
                    b'"' => in_string = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(offset);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(end) = end {
                out.push(&text[i..=end]);
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Run `tutti eval <skill-dir>`: load the skill and its eval records, drive each record through
/// a real `claude` session, judge each transcript semantically, and print a per-record line plus
/// a summary. Synchronous: it owns a runtime and the seams `block_on` it, so this MUST be called
/// from a thread that is not a runtime worker (see the module docs).
pub fn run(skill_dir: PathBuf, model: String) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    let skill = tutti_design::load_skill(&skill_dir).map_err(|e| e.to_string())?;
    let records = tutti_design::load_evals(&skill_dir).map_err(|e| e.to_string())?;
    if records.is_empty() {
        return Err(format!(
            "no eval records at {} (expected a non-empty evals.json)",
            skill_dir.join("evals.json").display()
        ));
    }

    // A throwaway working directory for the turns: the eval is not repo-bound.
    let workdir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let source = ClaudeTranscriptSource {
        session: ClaudeSession::default(),
        model: model.clone(),
        skill,
        cwd: workdir.path().to_path_buf(),
        handle: runtime.handle().clone(),
    };
    let judge = ClaudeJudge {
        session: ClaudeSession::default(),
        model,
        cwd: workdir.path().to_path_buf(),
        handle: runtime.handle().clone(),
    };

    let outcomes =
        tutti_design::run_evals_judged(&records, &source, &judge).map_err(|e| e.to_string())?;

    let mut passed = 0usize;
    for (record, outcome) in records.iter().zip(outcomes.iter()) {
        if outcome.passed {
            passed += 1;
        }
        let met: Vec<String> = record
            .expected_behavior
            .iter()
            .zip(outcome.met.iter())
            .map(|(behavior, &m)| format!("  [{}] {behavior}", if m { "x" } else { " " }))
            .collect();
        println!(
            "{} {}\n{}",
            if outcome.passed { "PASS" } else { "FAIL" },
            record.query,
            met.join("\n")
        );
    }
    println!("tutti: {passed}/{} eval record(s) passed", outcomes.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_objects_extracts_from_surrounding_prose() {
        let text = "Here is my verdict: {\"met\": [true, false]} and that is all.";
        assert_eq!(balanced_objects(text), vec!["{\"met\": [true, false]}"]);
    }

    #[test]
    fn balanced_objects_ignores_braces_inside_strings() {
        let text = "{\"note\": \"a } brace and a { brace\", \"met\": [true]}";
        assert_eq!(balanced_objects(text), vec![text]);
    }

    #[test]
    fn balanced_objects_empty_when_absent_or_unbalanced() {
        assert!(balanced_objects("no json here").is_empty());
        assert!(balanced_objects("{\"met\": [true]").is_empty());
    }

    #[test]
    fn balanced_objects_yields_a_prose_group_then_the_verdict() {
        // A chatty judge: an earlier brace group, then the real verdict. Both are returned in
        // order, so the judge can skip the first (which fails to parse) and use the second.
        let objs = balanced_objects("Looking at {behavior a, b}: {\"met\": [true, true]}");
        assert_eq!(objs.len(), 2);
        assert_eq!(objs[1], "{\"met\": [true, true]}");
    }

    #[test]
    fn verdict_deserializes_the_met_array() {
        let object = balanced_objects("prose {\"met\": [true, false, true]} more")
            .into_iter()
            .next()
            .unwrap();
        let verdict: Verdict = serde_json::from_str(object).unwrap();
        assert_eq!(verdict.met, vec![true, false, true]);
    }

    #[test]
    fn judging_prompt_numbers_the_behaviors_and_carries_the_transcript() {
        let prompt = judging_prompt(
            &[
                "Filtered the rows".to_string(),
                "Reported the count".to_string(),
            ],
            "the transcript body",
        );
        assert!(prompt.contains("1. Filtered the rows"));
        assert!(prompt.contains("2. Reported the count"));
        assert!(prompt.contains("the transcript body"));
        assert!(prompt.contains("\"met\""));
    }

    /// Live end-to-end: drive a real `claude` session over the Constitution skill's eval
    /// records and judge each transcript, asserting one verdict per record without error. It
    /// does not assert every record passes (a real judge may disagree with a record); it
    /// proves the live source + judge run end to end. Part of the `live` suite:
    /// `env -C <repo> cargo test -p tutti-cli --features live live_eval -- --nocapture`.
    ///
    /// A plain `#[test]` (not `#[tokio::test]`): the seams `block_on` a runtime, so the driving
    /// thread must not be a runtime worker.
    #[test]
    #[cfg_attr(not(feature = "live"), ignore = "live: needs claude -p on PATH")]
    fn live_eval_judges_the_constitution_skill() {
        let skill_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/design/constitution");
        let skill = tutti_design::load_skill(&skill_dir).expect("the constitution skill loads");
        let records = tutti_design::load_evals(&skill_dir).expect("its eval records load");
        assert!(
            records.len() >= 3,
            "the constitution skill carries at least three eval records"
        );

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let workdir = tempfile::tempdir().unwrap();
        let source = ClaudeTranscriptSource {
            session: ClaudeSession::default(),
            model: "sonnet".into(),
            skill,
            cwd: workdir.path().to_path_buf(),
            handle: runtime.handle().clone(),
        };
        let judge = ClaudeJudge {
            session: ClaudeSession::default(),
            model: "sonnet".into(),
            cwd: workdir.path().to_path_buf(),
            handle: runtime.handle().clone(),
        };

        let outcomes = tutti_design::run_evals_judged(&records, &source, &judge)
            .expect("the live source and judge run end to end");
        assert_eq!(outcomes.len(), records.len(), "one verdict per eval record");
        for (record, outcome) in records.iter().zip(outcomes.iter()) {
            assert_eq!(
                outcome.met.len(),
                record.expected_behavior.len(),
                "one boolean per expected behavior"
            );
            eprintln!(
                "{}: {} -> {:?}",
                if outcome.passed { "PASS" } else { "FAIL" },
                record.query,
                outcome.met
            );
        }
    }
}
