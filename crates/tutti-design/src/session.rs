// SPDX-License-Identifier: AGPL-3.0-or-later
//! The design session: which movements this session runs, and how far along it is.
//!
//! E1 advances a movement by ratifying it (the human's gate). Agent-facilitated
//! movements (a movement's mini-conversation) are E2; here `ratify` stands in for
//! "this movement's artifact section is done and the human approved it".

use crate::error::{DesignError, Result};
use crate::grounding::RepoGrounding;
use crate::movement::{movements_for, MovementId};
use crate::shape::ProjectShape;
use serde::{Deserialize, Serialize};

/// Who produced a turn in a movement's conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speaker {
    Agent,
    Human,
}

/// One line of a movement's conversation transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub speaker: Speaker,
    pub text: String,
}

/// The in-flight state of the movement currently being facilitated. Persisted after every
/// turn so a stop mid-movement resumes exactly where it left off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovementProgress {
    pub movement: MovementId,
    /// The agent conversation id to resume on the next turn (None before the first turn).
    pub agent_session_id: Option<String>,
    pub transcript: Vec<Turn>,
    /// The artifact section the agent has proposed (a `complete` reply), awaiting
    /// ratification. None while still asking questions.
    pub pending_artifact: Option<String>,
}

/// A ratified movement's captured artifact section (fed to the page renderer later).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovementArtifact {
    pub movement: MovementId,
    pub section: String,
}

/// The full, serializable state of a design session. This is exactly what is written to
/// `.tutti/design/session.json`, so resuming is just deserializing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    /// The project shape this session was created for.
    pub shape: ProjectShape,
    /// The selected movements, in the order they will run.
    pub movements: Vec<MovementId>,
    /// The movements already ratified, an in-order prefix of `movements`.
    pub ratified: Vec<MovementId>,
    /// The movement being facilitated right now, if any (in-flight conversation state).
    #[serde(default)]
    pub active: Option<MovementProgress>,
    /// Ratified movements' artifact sections, in ratification order.
    #[serde(default)]
    pub artifacts: Vec<MovementArtifact>,
    /// A read-only summary of the existing repo this session designs against, when the
    /// session was grounded in one. `None` for a greenfield session.
    #[serde(default)]
    pub grounding: Option<RepoGrounding>,
}

impl SessionState {
    /// Start a fresh session for a project shape. The rails are selected once, up front,
    /// so a mid-session change of shape is a new session, not a mutation.
    pub fn new(shape: ProjectShape) -> Self {
        Self {
            shape,
            movements: movements_for(shape),
            ratified: Vec::new(),
            active: None,
            artifacts: Vec::new(),
            grounding: None,
        }
    }

    /// The movement currently awaiting work, or `None` when the session is complete.
    pub fn current(&self) -> Option<MovementId> {
        self.movements.get(self.ratified.len()).copied()
    }

    /// Ratify the current movement (the gate), advancing to the next. Returns the
    /// movement that was ratified. Errors if the session is already complete.
    pub fn ratify(&mut self) -> Result<MovementId> {
        match self.current() {
            Some(m) => {
                self.ratified.push(m);
                Ok(m)
            }
            None => Err(DesignError::AlreadyComplete),
        }
    }

    /// True once every selected movement has been ratified.
    pub fn is_complete(&self) -> bool {
        self.ratified.len() == self.movements.len()
    }

    /// Check the session's internal consistency: `ratified` must be no longer than
    /// `movements`, and must be an in-order prefix of it.
    ///
    /// This deliberately does NOT re-derive `movements` from `shape` via
    /// `movements_for`, so a session persisted under an older branching rule still
    /// loads; it only checks that the two fields already on the struct agree with each
    /// other, not that they agree with the current code's selection rule.
    pub fn validate(&self) -> Result<()> {
        if self.movements.is_empty() {
            return Err(DesignError::Corrupt(
                "movements is empty; a session must run at least one movement".to_string(),
            ));
        }
        if self.ratified.len() > self.movements.len() {
            return Err(DesignError::Corrupt(format!(
                "ratified has {} entries, more than movements' {}",
                self.ratified.len(),
                self.movements.len()
            )));
        }
        if self.movements[..self.ratified.len()] != self.ratified[..] {
            return Err(DesignError::Corrupt(
                "ratified is not an in-order prefix of movements".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_starts_at_the_first_selected_movement() {
        let s = SessionState::new(ProjectShape::SmallCli);
        assert_eq!(s.current(), Some(MovementId::Constitution));
        assert!(!s.is_complete());
    }

    #[test]
    fn ratifying_walks_the_chain_in_order_to_completion() {
        let mut s = SessionState::new(ProjectShape::SmallCli);
        let expected = movements_for(ProjectShape::SmallCli);
        let mut walked = Vec::new();
        while s.current().is_some() {
            walked.push(s.ratify().unwrap());
        }
        assert_eq!(walked, expected);
        assert!(s.is_complete());
        assert_eq!(s.current(), None);
    }

    #[test]
    fn ratifying_a_complete_session_errors() {
        let mut s = SessionState::new(ProjectShape::SmallCli);
        while s.current().is_some() {
            s.ratify().unwrap();
        }
        assert!(matches!(s.ratify(), Err(DesignError::AlreadyComplete)));
    }

    #[test]
    fn a_partially_walked_session_resumes_at_the_right_movement() {
        let mut s = SessionState::new(ProjectShape::MultiService);
        s.ratify().unwrap(); // Constitution
        s.ratify().unwrap(); // Frame
                             // Serialize and deserialize to prove state fully round-trips (persistence is Task 6).
        let json = serde_json::to_string(&s).unwrap();
        let resumed: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(resumed.current(), Some(MovementId::Impact));
        assert_eq!(resumed.ratified.len(), 2);
    }

    #[test]
    fn validate_rejects_an_empty_movement_list() {
        // A session that runs no movements is degenerate: it would report itself
        // complete without ever doing work. A valid session always has movements
        // (movements_for never returns an empty set), so an empty list means corruption.
        let s = SessionState {
            shape: ProjectShape::SmallCli,
            movements: Vec::new(),
            ratified: Vec::new(),
            active: None,
            artifacts: Vec::new(),
            grounding: None,
        };
        assert!(matches!(s.validate(), Err(DesignError::Corrupt(_))));
    }

    #[test]
    fn validate_accepts_a_well_formed_session() {
        let mut s = SessionState::new(ProjectShape::MultiService);
        s.ratify().unwrap();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn a_fresh_session_has_no_active_movement_and_no_artifacts() {
        let s = SessionState::new(ProjectShape::SmallCli);
        assert!(s.active.is_none());
        assert!(s.artifacts.is_empty());
    }

    #[test]
    fn session_state_roundtrips_active_and_artifacts_through_serde() {
        let mut s = SessionState::new(ProjectShape::SmallCli);
        s.active = Some(MovementProgress {
            movement: MovementId::Constitution,
            agent_session_id: Some("sid-1".into()),
            transcript: vec![Turn {
                speaker: Speaker::Agent,
                text: "q?".into(),
            }],
            pending_artifact: None,
        });
        s.artifacts.push(MovementArtifact {
            movement: MovementId::Constitution,
            section: "principles: ...".into(),
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
