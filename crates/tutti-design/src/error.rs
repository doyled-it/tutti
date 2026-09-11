// SPDX-License-Identifier: AGPL-3.0-or-later
//! The crate's error type.

use thiserror::Error;

/// Anything that can go wrong walking or persisting a design session.
#[derive(Debug, Error)]
pub enum DesignError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    /// `ratify` was called on a session that has already reached its terminal movement.
    #[error("session already complete")]
    AlreadyComplete,
    /// A loaded session failed its structural invariants (see `SessionState::validate`).
    #[error("corrupt session: {0}")]
    Corrupt(String),
    /// A SKILL.md skill could not be loaded or parsed (missing file, malformed frontmatter).
    #[error("skill: {0}")]
    Skill(String),
    /// A forge operation failed while seeding the backlog.
    #[error("forge: {0}")]
    Forge(String),
    /// A movement's facilitation loop could not proceed (a malformed agent reply, or an
    /// invalid step such as ratifying before an artifact was proposed).
    #[error("facilitation: {0}")]
    Facilitation(String),
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, DesignError>;
