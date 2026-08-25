// SPDX-License-Identifier: AGPL-3.0-or-later
//! The design session: which movements this session runs, and how far along it is.
//!
//! E1 advances a movement by ratifying it (the human's gate). Agent-facilitated
//! movements (a movement's mini-conversation) are E2; here `ratify` stands in for
//! "this movement's artifact section is done and the human approved it".

use crate::error::{DesignError, Result};
use crate::movement::{movements_for, MovementId};
use crate::shape::ProjectShape;
use serde::{Deserialize, Serialize};

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
}

impl SessionState {
    /// Start a fresh session for a project shape. The rails are selected once, up front,
    /// so a mid-session change of shape is a new session, not a mutation.
    pub fn new(shape: ProjectShape) -> Self {
        Self {
            shape,
            movements: movements_for(shape),
            ratified: Vec::new(),
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
        while let Some(_) = s.current() {
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
}
