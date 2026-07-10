// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tutti engine core: the offline rails loop, testable with fakes.

pub mod domain;
pub mod message;
pub mod testing;
pub mod traits;

/// The engine's semantic version, surfaced in logs and handoff artifacts.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod smoke {
    #[test]
    fn version_is_present() {
        assert!(!super::VERSION.is_empty());
    }
}
