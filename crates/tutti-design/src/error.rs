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
}

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, DesignError>;
