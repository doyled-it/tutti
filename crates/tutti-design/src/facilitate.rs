// SPDX-License-Identifier: AGPL-3.0-or-later
//! The single-movement facilitation loop: a consumer-owned conversation seam, the tagged
//! agent reply and its parser, and the step function that drives one movement to
//! ratification.

use crate::error::{DesignError, Result};
use crate::movement::{definition, MovementId};
use crate::session::{MovementArtifact, MovementProgress, SessionState, Speaker, Turn};
use crate::skill::Skill;
use crate::store;
use serde::Deserialize;
use std::path::Path;

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

/// Parse a tagged reply from the agent's raw output. A `claude -p` reply may wrap the object
/// in prose or a fenced code block, and the prose may itself contain balanced braces, so this
/// tries each top-level balanced `{...}` object in order and returns the first that is a valid
/// tagged reply. A reply carrying neither tag, both tags, or an empty `ask`/`artifact_section`
/// is rejected, never a silent pass.
pub fn parse_reply(output: &str) -> Result<MovementReply> {
    let mut last_err: Option<DesignError> = None;
    for candidate in balanced_objects(output) {
        let Ok(wire) = serde_json::from_str::<ReplyWire>(candidate) else {
            continue; // not a reply object (e.g. balanced prose braces); try the next one
        };
        match interpret(wire) {
            Ok(reply) => return Ok(reply),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        DesignError::Facilitation("no valid reply JSON object in the agent reply".into())
    }))
}

/// Turn a parsed `ReplyWire` into a `MovementReply`, enforcing exactly one non-empty tag.
fn interpret(wire: ReplyWire) -> Result<MovementReply> {
    match (wire.ask, wire.complete) {
        (Some(question), None) => {
            if question.trim().is_empty() {
                return Err(DesignError::Facilitation("`ask` was empty".into()));
            }
            Ok(MovementReply::Ask { question })
        }
        (None, Some(body)) => {
            if body.artifact_section.trim().is_empty() {
                return Err(DesignError::Facilitation(
                    "`complete.artifact_section` was empty".into(),
                ));
            }
            Ok(MovementReply::Complete {
                artifact_section: body.artifact_section,
                diagrams: body.diagrams,
            })
        }
        (Some(_), Some(_)) => Err(DesignError::Facilitation(
            "reply carried both `ask` and `complete`".into(),
        )),
        (None, None) => Err(DesignError::Facilitation(
            "reply carried neither `ask` nor `complete`".into(),
        )),
    }
}

/// Every top-level balanced `{...}` object in `s`, in order, respecting string literals and
/// escapes (so a brace inside a JSON string does not throw off the balance). Nested objects
/// are not yielded separately; each returned slice is a complete top-level object. Slices
/// start and end on `{`/`}` (both ASCII), so they are always valid char boundaries.
fn balanced_objects(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = balanced_end(bytes, i) {
                out.push(&s[i..=end]);
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// The index of the `}` that closes the `{` at `start`, or None if unbalanced.
fn balanced_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// What the human is feeding into the loop this call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacilitationInput {
    /// Start the movement (no active conversation yet).
    Begin,
    /// The human's answer to the agent's last question.
    Reply(String),
    /// Ratify the proposed artifact section (advance the movement).
    Accept,
    /// Reject the proposed artifact and ask the agent to revise, with feedback.
    Revise(String),
}

/// What to present to the human after one `advance`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacilitationState {
    /// The agent asked a question; show it and collect a `Reply`.
    AwaitingHuman { question: String },
    /// The agent proposed an artifact section; show it and collect `Accept`/`Revise`.
    AwaitingRatification { artifact_section: String },
    /// The movement was ratified; `next` is the following movement, if any.
    Ratified {
        movement: MovementId,
        next: Option<MovementId>,
    },
}

/// Build the prompt for one turn: the movement skill body, the guiding question, the coverage
/// checklist, the reply-format contract, and the human's latest input.
fn build_turn_prompt(skill: &Skill, movement: MovementId, human_input: &str) -> String {
    let def = definition(movement);
    let checklist = def
        .checklist
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{skill_body}\n\n\
         You are facilitating the \"{title}\" movement. Guiding question: {question}\n\n\
         Cover these points, going deeper where the answer is novel or complex:\n{checklist}\n\n\
         Ask ONE question at a time. Reply with a single JSON object and nothing else: \
         either {{\"ask\": \"<your next question>\"}} while you still need input, or \
         {{\"complete\": {{\"artifact_section\": \"<the movement's section as markdown>\", \
         \"diagrams\": []}}}} once every point is covered.\n\n\
         Human: {human_input}",
        skill_body = skill.body,
        title = def.title,
        question = def.guiding_question,
    )
}

/// Advance the facilitation of `movement` by one interaction. Persists `session` after every
/// mutation, so a stop between calls loses nothing.
pub async fn advance(
    fac: &impl Facilitator,
    skill: &Skill,
    movement: MovementId,
    session: &mut SessionState,
    repo_root: &Path,
    input: FacilitationInput,
) -> Result<FacilitationState> {
    // `movement` must be the session's current (next-unratified) movement, so a stale or
    // desynced caller cannot ratify or facilitate a movement other than the one in flight.
    if session.current() != Some(movement) {
        return Err(DesignError::Facilitation(
            "movement is not the session's current movement".into(),
        ));
    }
    // If a conversation is already in flight, it must be for this same movement.
    if let Some(p) = session.active.as_ref() {
        if p.movement != movement {
            return Err(DesignError::Facilitation(
                "the active conversation is for a different movement".into(),
            ));
        }
    }
    match input {
        FacilitationInput::Accept => {
            let progress = session
                .active
                .as_ref()
                .ok_or_else(|| DesignError::Facilitation("no active movement to ratify".into()))?;
            let section = progress.pending_artifact.clone().ok_or_else(|| {
                DesignError::Facilitation(
                    "cannot ratify: the movement has no proposed artifact yet".into(),
                )
            })?;
            session
                .artifacts
                .push(MovementArtifact { movement, section });
            session.ratify()?; // advances the ratified prefix (E1 invariant checks)
            session.active = None;
            store::save(repo_root, session)?;
            let next = session.current();
            Ok(FacilitationState::Ratified { movement, next })
        }
        FacilitationInput::Begin | FacilitationInput::Reply(_) | FacilitationInput::Revise(_) => {
            // The human's text for this turn (empty on Begin).
            let human_input = match &input {
                FacilitationInput::Reply(t) | FacilitationInput::Revise(t) => t.clone(),
                _ => String::new(),
            };
            // Resume the in-flight conversation if one exists (None on the first turn).
            let resume = session
                .active
                .as_ref()
                .and_then(|p| p.agent_session_id.clone());
            // Run the turn BEFORE mutating any state, so a backend error or an unparseable
            // reply leaves `session` untouched (no half-written transcript, no desynced
            // resume id) and a retry with the same session is clean.
            let prompt = build_turn_prompt(skill, movement, &human_input);
            let raw = fac.turn(&prompt, resume.as_deref()).await?;
            let reply = parse_reply(&raw.output)?;
            // Success: now mutate. Create the progress record on the first turn.
            let progress = session.active.get_or_insert_with(|| MovementProgress {
                movement,
                agent_session_id: None,
                transcript: vec![],
                pending_artifact: None,
            });
            progress.agent_session_id = Some(raw.session_id);
            if !human_input.is_empty() {
                progress.transcript.push(Turn {
                    speaker: Speaker::Human,
                    text: human_input,
                });
            }
            match reply {
                MovementReply::Ask { question } => {
                    progress.transcript.push(Turn {
                        speaker: Speaker::Agent,
                        text: question.clone(),
                    });
                    progress.pending_artifact = None;
                    store::save(repo_root, session)?;
                    Ok(FacilitationState::AwaitingHuman { question })
                }
                MovementReply::Complete {
                    artifact_section,
                    diagrams: _,
                } => {
                    progress.transcript.push(Turn {
                        speaker: Speaker::Agent,
                        text: artifact_section.clone(),
                    });
                    progress.pending_artifact = Some(artifact_section.clone());
                    store::save(repo_root, session)?;
                    Ok(FacilitationState::AwaitingRatification { artifact_section })
                }
            }
        }
    }
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
        let r = parse_reply(
            "Sure!\n```json\n{\"ask\": \"What is out of scope?\"}\n```\nhope that helps",
        )
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

    struct ScriptedFacilitator {
        replies: std::cell::RefCell<Vec<String>>,
        /// The `resume` argument each `turn` call received, in order (for threading tests).
        resumes: std::cell::RefCell<Vec<Option<String>>>,
    }
    impl ScriptedFacilitator {
        fn new(replies: Vec<&str>) -> Self {
            Self {
                replies: std::cell::RefCell::new(replies.into_iter().map(String::from).collect()),
                resumes: std::cell::RefCell::new(vec![]),
            }
        }
    }
    impl Facilitator for ScriptedFacilitator {
        async fn turn(&self, _prompt: &str, resume: Option<&str>) -> Result<RawTurn> {
            self.resumes.borrow_mut().push(resume.map(String::from));
            let out = self.replies.borrow_mut().remove(0);
            Ok(RawTurn {
                session_id: "sid".into(),
                output: out,
            })
        }
    }

    fn constitution_skill_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/design/constitution")
    }

    #[test]
    fn constitution_skill_loads_and_passes_the_structural_lint() {
        let skill = crate::skill::load(&constitution_skill_dir()).expect("skill loads");
        assert_eq!(skill.name, "design-constitution");
        assert!(
            crate::lint::lint(&skill).is_empty(),
            "lint: {:?}",
            crate::lint::lint(&skill)
        );
        let evals = crate::eval::load_evals(&constitution_skill_dir()).expect("evals load");
        assert!(crate::eval::has_minimum_evals(&evals));
    }

    fn constitution_skill() -> Skill {
        // A minimal in-memory Skill is enough for the loop; the body is injected into the prompt.
        Skill {
            name: "design-constitution".into(),
            description: "Facilitate the Constitution movement.".into(),
            body: "Ask about principles and non-negotiables. When both are covered, emit complete."
                .into(),
            dir: std::path::PathBuf::from("skills/design/constitution"),
            references: vec![],
            scripts: vec![],
        }
    }

    #[tokio::test]
    async fn begin_then_answer_then_complete_then_ratify_advances_the_movement() {
        let d = tempfile::tempdir().unwrap();
        let fac = ScriptedFacilitator::new(vec![
            r#"{"ask": "What must stay true?"}"#,
            r###"{"complete": {"artifact_section": "## Constitution\nprivacy first", "diagrams": []}}"###,
        ]);
        let mut s = SessionState::new(crate::shape::ProjectShape::SmallCli);
        let m = MovementId::Constitution;

        // Begin: first turn asks.
        let st = advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Begin,
        )
        .await
        .unwrap();
        assert!(matches!(st, FacilitationState::AwaitingHuman { .. }));
        assert!(s.active.is_some(), "active movement persisted");

        // Answer: second turn completes.
        let st = advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Reply("privacy".into()),
        )
        .await
        .unwrap();
        match st {
            FacilitationState::AwaitingRatification {
                ref artifact_section,
            } => {
                assert!(artifact_section.contains("privacy first"))
            }
            _ => panic!("expected AwaitingRatification"),
        }

        // Ratify: movement moves into ratified + artifacts, active cleared.
        let st = advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Accept,
        )
        .await
        .unwrap();
        assert!(matches!(
            st,
            FacilitationState::Ratified {
                movement: MovementId::Constitution,
                ..
            }
        ));
        assert!(s.ratified.contains(&MovementId::Constitution));
        assert_eq!(s.artifacts.len(), 1);
        assert!(s.active.is_none());

        // The whole thing persisted: a reload sees the ratified movement.
        let reloaded = store::load(d.path()).unwrap().unwrap();
        assert!(reloaded.ratified.contains(&MovementId::Constitution));
    }

    #[tokio::test]
    async fn accept_without_a_pending_artifact_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let fac = ScriptedFacilitator::new(vec![r#"{"ask":"q"}"#]);
        let mut s = SessionState::new(crate::shape::ProjectShape::SmallCli);
        advance(
            &fac,
            &constitution_skill(),
            MovementId::Constitution,
            &mut s,
            d.path(),
            FacilitationInput::Begin,
        )
        .await
        .unwrap();
        // Only a question so far; accepting is invalid.
        assert!(advance(
            &fac,
            &constitution_skill(),
            MovementId::Constitution,
            &mut s,
            d.path(),
            FacilitationInput::Accept,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn revise_after_complete_runs_another_turn() {
        let d = tempfile::tempdir().unwrap();
        let fac = ScriptedFacilitator::new(vec![
            r#"{"complete": {"artifact_section": "v1"}}"#,
            r#"{"complete": {"artifact_section": "v2 revised"}}"#,
        ]);
        let mut s = SessionState::new(crate::shape::ProjectShape::SmallCli);
        let m = MovementId::Constitution;
        advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Begin,
        )
        .await
        .unwrap();
        let st = advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Revise("tighten it".into()),
        )
        .await
        .unwrap();
        match st {
            FacilitationState::AwaitingRatification { artifact_section } => {
                assert!(artifact_section.contains("v2 revised"))
            }
            _ => panic!("expected AwaitingRatification after revise"),
        }
    }

    // Regression tests for the adversarial review findings.

    #[test]
    fn empty_ask_and_empty_artifact_are_rejected() {
        assert!(parse_reply(r#"{"ask": ""}"#).is_err());
        assert!(parse_reply(r#"{"ask": "   "}"#).is_err());
        assert!(parse_reply(r#"{"complete": {"artifact_section": ""}}"#).is_err());
    }

    #[test]
    fn parse_reply_skips_balanced_prose_braces_before_the_real_object() {
        let r = parse_reply("see {details here} then: {\"ask\": \"go?\"}").unwrap();
        assert_eq!(
            r,
            MovementReply::Ask {
                question: "go?".into()
            }
        );
    }

    #[tokio::test]
    async fn the_second_turn_resumes_with_the_first_turns_session_id() {
        let d = tempfile::tempdir().unwrap();
        let fac = ScriptedFacilitator::new(vec![
            r#"{"ask": "q1?"}"#,
            r#"{"complete": {"artifact_section": "done"}}"#,
        ]);
        let mut s = SessionState::new(crate::shape::ProjectShape::SmallCli);
        let m = MovementId::Constitution;
        advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Begin,
        )
        .await
        .unwrap();
        advance(
            &fac,
            &constitution_skill(),
            m,
            &mut s,
            d.path(),
            FacilitationInput::Reply("a1".into()),
        )
        .await
        .unwrap();
        assert_eq!(
            *fac.resumes.borrow(),
            vec![None, Some("sid".to_string())],
            "first turn has no resume; the second resumes the first turn's session id"
        );
    }

    #[tokio::test]
    async fn advancing_a_movement_that_is_not_current_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let fac = ScriptedFacilitator::new(vec![r#"{"ask":"q"}"#]);
        let mut s = SessionState::new(crate::shape::ProjectShape::SmallCli);
        // Constitution is current, not Frame: advancing Frame must error, not silently
        // facilitate or ratify the wrong movement.
        let out = advance(
            &fac,
            &constitution_skill(),
            MovementId::Frame,
            &mut s,
            d.path(),
            FacilitationInput::Begin,
        )
        .await;
        assert!(out.is_err());
        assert!(
            s.active.is_none(),
            "no state was mutated for the wrong movement"
        );
    }
}
