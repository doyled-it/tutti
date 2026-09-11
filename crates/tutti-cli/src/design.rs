// SPDX-License-Identifier: AGPL-3.0-or-later
//! `tutti design`: an interactive CLI that drives the design chain (the E2/E5 facilitation
//! loop) to a ratified design, grounds an existing repo first, then turns the ratified design
//! into a seedable backlog via a post-chain decompose pass and hands off (new repo: scaffold +
//! seed; existing repo: seed). Resumable and branchable from `.tutti/design/`.
//!
//! The wiring lives here. The real `Facilitator` is a thin adapter over `ClaudeSession::turn`
//! (`ClaudeFacilitator`); the interactive driver is written over an injected `Prompter` and the
//! `Facilitator` seam so it is hermetically testable with a fake of each. The full chain against
//! a real `claude` is a live `#[ignore]` smoke.

use std::path::{Path, PathBuf};
use tutti_backend_claude::session::ClaudeSession;
use tutti_core::message::AgentEvent;
use tutti_design::{
    advance, DesignError, FacilitationInput, FacilitationState, Facilitator, MovementId, RawTurn,
    SessionState, Skill,
};

/// The real `Facilitator`: one facilitation turn is one `ClaudeSession::turn` in `cwd`,
/// resuming the session id from the prior turn. The streamed agent events are drained to a
/// sink for now (a later surface can show them live); the turn's outcome is its full reply
/// text plus the resumable session id, which is exactly what a `RawTurn` carries.
pub struct ClaudeFacilitator {
    pub session: ClaudeSession,
    pub model: String,
    pub cwd: PathBuf,
}

impl Facilitator for ClaudeFacilitator {
    async fn turn(&self, prompt: &str, resume: Option<&str>) -> Result<RawTurn, DesignError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
        // Drain the streamed events so the bounded channel never backs up the turn. A live
        // surface can replace this sink with a real renderer later.
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let outcome = self
            .session
            .turn(prompt, &self.model, resume, None, &self.cwd, None, tx)
            .await
            .map_err(|e| DesignError::Facilitation(format!("claude turn: {e}")))?;
        let _ = drain.await;
        Ok(RawTurn {
            session_id: outcome.session_id.unwrap_or_default(),
            output: outcome.assistant_text,
        })
    }
}

/// Resolve the `skills/` directory that holds the movement facilitation skills. One resolver so
/// the shipped-skills packaging (bundled beside the binary, like codegraph) has a single place to
/// change: the `TUTTI_SKILLS_DIR` env override wins, else a `skills/` dir beside the running
/// binary, else the dev/test fallback of `CARGO_MANIFEST_DIR/../../skills` (the repo's own).
fn skills_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TUTTI_SKILLS_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let beside = parent.join("skills");
            if beside.is_dir() {
                return beside;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills")
}

/// Load a movement's facilitation skill from `skills/design/<name>/`, via the single
/// `skills_dir` resolver.
pub fn movement_skill(id: MovementId) -> Result<Skill, DesignError> {
    let dir = skills_dir()
        .join("design")
        .join(tutti_design::skill_dir_name(id));
    tutti_design::load_skill(&dir)
}

/// The human side of the loop, behind a trait so the driver is testable with a scripted fake.
pub trait Prompter {
    /// Show `question` and read one line of input.
    fn ask(&mut self, question: &str) -> std::io::Result<String>;
    /// Display text to the human (a proposed artifact, a status line).
    fn show(&mut self, text: &str);
    /// Ask a yes/no question, defaulting to no.
    fn confirm(&mut self, prompt: &str) -> std::io::Result<bool>;
}

/// Drive one movement to ratification over the `Facilitator` seam and the injected `Prompter`,
/// persisting after each step (`advance` writes the session). The loop turns each
/// `FacilitationState` into the next human input: a question is asked and answered, a proposed
/// artifact is shown and either ratified or sent back for revision, and ratification ends it.
pub async fn drive_movement(
    fac: &impl Facilitator,
    prompter: &mut impl Prompter,
    skill: &Skill,
    movement: MovementId,
    session: &mut SessionState,
    repo_root: &Path,
) -> Result<(), DesignError> {
    let mut input = FacilitationInput::Begin;
    loop {
        let state = advance(fac, skill, movement, session, repo_root, input).await?;
        input = match state {
            FacilitationState::AwaitingHuman { question } => {
                let answer = prompter
                    .ask(&question)
                    .map_err(|e| DesignError::Facilitation(format!("reading input: {e}")))?;
                FacilitationInput::Reply(answer)
            }
            FacilitationState::AwaitingRatification { artifact_section } => {
                prompter.show(&artifact_section);
                let ratify = prompter
                    .confirm("Ratify this section?")
                    .map_err(|e| DesignError::Facilitation(format!("reading input: {e}")))?;
                if ratify {
                    FacilitationInput::Accept
                } else {
                    let feedback = prompter
                        .ask("What should change?")
                        .map_err(|e| DesignError::Facilitation(format!("reading input: {e}")))?;
                    FacilitationInput::Revise(feedback)
                }
            }
            FacilitationState::Ratified { .. } => return Ok(()),
        };
    }
}

/// Drive the whole chain to completion: for each remaining movement, load its facilitation
/// skill and `drive_movement` it to ratification, until `session.is_complete()`. State is
/// persisted throughout by `advance`, so a stop between movements resumes here.
pub async fn drive_chain(
    fac: &impl Facilitator,
    prompter: &mut impl Prompter,
    session: &mut SessionState,
    repo_root: &Path,
) -> Result<(), DesignError> {
    while let Some(movement) = session.current() {
        let skill = movement_skill(movement)?;
        drive_movement(fac, prompter, &skill, movement, session, repo_root).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tutti_design::{store, ProjectShape};

    /// A scripted facilitator: hands back the queued replies in order. The same shape E2 uses
    /// for its hermetic loop tests.
    struct FakeFacilitator {
        replies: std::cell::RefCell<VecDeque<String>>,
    }
    impl FakeFacilitator {
        fn new(replies: Vec<&str>) -> Self {
            Self {
                replies: std::cell::RefCell::new(replies.into_iter().map(String::from).collect()),
            }
        }
    }
    impl Facilitator for FakeFacilitator {
        async fn turn(&self, _prompt: &str, _resume: Option<&str>) -> Result<RawTurn, DesignError> {
            let out = self
                .replies
                .borrow_mut()
                .pop_front()
                .expect("FakeFacilitator ran out of scripted replies");
            Ok(RawTurn {
                session_id: "sid".into(),
                output: out,
            })
        }
    }

    /// A scripted prompter: answers and confirmations dequeued in order; `shown` records what
    /// the driver displayed.
    struct FakePrompter {
        answers: VecDeque<String>,
        confirms: VecDeque<bool>,
        shown: Vec<String>,
    }
    impl FakePrompter {
        fn new(answers: Vec<&str>, confirms: Vec<bool>) -> Self {
            Self {
                answers: answers.into_iter().map(String::from).collect(),
                confirms: confirms.into_iter().collect(),
                shown: Vec::new(),
            }
        }
    }
    impl Prompter for FakePrompter {
        fn ask(&mut self, _question: &str) -> std::io::Result<String> {
            Ok(self
                .answers
                .pop_front()
                .expect("FakePrompter ran out of scripted answers"))
        }
        fn show(&mut self, text: &str) {
            self.shown.push(text.to_string());
        }
        fn confirm(&mut self, _prompt: &str) -> std::io::Result<bool> {
            Ok(self
                .confirms
                .pop_front()
                .expect("FakePrompter ran out of scripted confirmations"))
        }
    }

    #[tokio::test]
    async fn driver_runs_one_movement_to_ratification() {
        let repo = tempfile::tempdir().unwrap();
        // The agent asks once, then completes with the Constitution section.
        let fac = FakeFacilitator::new(vec![
            r#"{"ask": "What must stay true?"}"#,
            r###"{"complete": {"artifact_section": "## Constitution\nprivacy first"}}"###,
        ]);
        // The human answers the question, then ratifies.
        let mut prompter = FakePrompter::new(vec!["privacy first"], vec![true]);

        // The first movement of a SmallCli session is Constitution; load its real skill.
        let mut session = SessionState::new(ProjectShape::SmallCli);
        let movement = session.current().expect("a fresh session has a movement");
        assert_eq!(movement, MovementId::Constitution);
        let skill = movement_skill(movement).expect("the Constitution skill loads");

        drive_movement(
            &fac,
            &mut prompter,
            &skill,
            movement,
            &mut session,
            repo.path(),
        )
        .await
        .expect("the movement drives to ratification");

        // The movement is ratified in the session and the proposed artifact was shown.
        assert!(session.ratified.contains(&MovementId::Constitution));
        assert_eq!(session.artifacts.len(), 1);
        assert!(session.active.is_none());
        assert!(prompter.shown.iter().any(|s| s.contains("privacy first")));

        // It persisted: a fresh load from disk sees the ratified movement.
        let reloaded = store::load(repo.path()).unwrap().expect("a saved session");
        assert!(reloaded.ratified.contains(&MovementId::Constitution));
    }

    #[tokio::test]
    async fn a_rejected_section_is_revised_then_ratified() {
        let repo = tempfile::tempdir().unwrap();
        // The agent completes, is asked to revise, then completes again.
        let fac = FakeFacilitator::new(vec![
            r###"{"complete": {"artifact_section": "## Constitution\nv1"}}"###,
            r###"{"complete": {"artifact_section": "## Constitution\nv2 tightened"}}"###,
        ]);
        // Reject the first section (with revision feedback), ratify the second.
        let mut prompter = FakePrompter::new(vec!["tighten it"], vec![false, true]);

        let mut session = SessionState::new(ProjectShape::SmallCli);
        let movement = session.current().unwrap();
        let skill = movement_skill(movement).unwrap();
        drive_movement(
            &fac,
            &mut prompter,
            &skill,
            movement,
            &mut session,
            repo.path(),
        )
        .await
        .unwrap();

        assert!(session.ratified.contains(&MovementId::Constitution));
        // The ratified artifact is the revised one.
        assert!(session.artifacts[0].section.contains("v2 tightened"));
    }
}
