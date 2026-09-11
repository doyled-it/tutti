// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tutti Score: the app-level design chain ("design on rails"). E1 is the hermetic
//! core: the movement model, project-shape branching, the session state machine, and
//! resumable on-disk state. E1.5 adds the hermetic half of the skill system: loading
//! and representing an Anthropic-style SKILL.md skill directory, a structural lint over
//! the authoring rules, and an evaluation-record model with deterministic proxy scoring
//! and a runner over a transcript-source seam. E6 adds the forge decomposer
//! (`decompose`): a `BacklogPlan` model for the milestone/epic/issue tree a Score
//! design proposes, a deterministic `render_plan` for the propose -> review step, and
//! an idempotent `seed` that creates the tree through `tutti-core`'s `Forge` seam,
//! re-runnable without duplicating work. E3 adds the design-page renderer: `svg`
//! ports the Sotto house-style inline-SVG diagram vocabulary (the page CSS, the
//! node/zone/edge primitive builders, a `Diagram` composer, and a well-formedness
//! check), and `page` accretes a `DesignPage` of ratified sections into a single
//! self-contained HTML document via `render_page`. Generating section content from a
//! facilitated session is out of scope here; this module only renders what it is
//! given.

pub mod decompose;
#[cfg(test)]
mod diagrams;
pub mod error;
pub mod eval;
pub mod facilitate;
pub mod grounding;
pub mod lint;
pub mod movement;
pub mod page;
pub mod session;
pub mod shape;
pub mod skill;
pub mod store;
pub mod svg;

pub use decompose::{
    render_plan, seed, BacklogPlan, ProposedEpic, ProposedIssue, ProposedMilestone, SeedReport,
};
pub use error::{DesignError, Result};
pub use eval::{load_evals, run_evals, score, EvalOutcome, EvalRecord, SkillTranscriptSource};
pub use facilitate::{
    advance, parse_reply, FacilitationInput, FacilitationState, Facilitator, MovementReply, RawTurn,
};
pub use grounding::{DomainSignal, RepoGrounder, RepoGrounding};
pub use lint::{lint, Violation};
pub use movement::{definition, movements_for, skill_dir_name, Movement, MovementId, RAILS};
pub use page::{render_page, DesignPage, Section};
pub use session::SessionState;
pub use shape::ProjectShape;
pub use skill::{load as load_skill, Frontmatter, Skill};
pub use svg::{edge, is_well_formed_svg, node, zone, Accent, Diagram, PAGE_CSS};

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
