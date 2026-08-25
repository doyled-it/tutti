// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tutti Score: the app-level design chain ("design on rails"). E1 is the hermetic
//! core: the movement model, project-shape branching, the session state machine, and
//! resumable on-disk state.

pub mod error;
pub mod movement;
pub mod session;
pub mod shape;

/// The crate's semantic version, surfaced in artifacts later.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod smoke {
    #[test]
    fn version_is_present() {
        assert!(!super::VERSION.is_empty());
    }
}
