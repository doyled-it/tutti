// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tutti Score: the app-level design chain ("design on rails"). E1 is the hermetic
//! core: the movement model, project-shape branching, the session state machine, and
//! resumable on-disk state.

pub mod error;
pub mod eval;
pub mod lint;
pub mod movement;
pub mod session;
pub mod shape;
pub mod skill;
pub mod store;

pub use error::{DesignError, Result};
pub use eval::{load_evals, run_evals, score, EvalOutcome, EvalRecord, SkillTranscriptSource};
pub use lint::{lint, Violation};
pub use movement::{definition, movements_for, Movement, MovementId, RAILS};
pub use session::SessionState;
pub use shape::ProjectShape;
pub use skill::{load as load_skill, Frontmatter, Skill};

/// The crate's semantic version, surfaced in artifacts later.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod smoke {
    #[test]
    fn version_is_present() {
        assert!(!super::VERSION.is_empty());
    }
}

#[cfg(test)]
mod integration {
    use super::*;

    #[test]
    fn a_full_session_walks_persists_and_resumes_to_completion() {
        let repo = tempfile::tempdir().unwrap();

        // Create a small-CLI session and persist it fresh.
        let mut s = SessionState::new(ProjectShape::SmallCli);
        store::save(repo.path(), &s).unwrap();

        // Walk it one movement at a time, persisting after each ratify, and prove that a
        // fresh load from disk resumes at exactly the same point every step.
        while s.current().is_some() {
            let ratified = s.ratify().unwrap();
            store::save(repo.path(), &s).unwrap();
            let resumed = store::load(repo.path()).unwrap().unwrap();
            assert_eq!(resumed, s);
            // The just-ratified movement is one of the shape's selected movements.
            assert!(movements_for(ProjectShape::SmallCli).contains(&ratified));
        }

        assert!(s.is_complete());
        assert!(store::load(repo.path()).unwrap().unwrap().is_complete());
    }
}
