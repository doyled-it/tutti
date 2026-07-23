// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tutti engine core: the offline rails loop, testable with fakes.

pub mod browse;
pub mod config;
pub mod domain;
pub mod engine;
pub mod events;
pub mod executor;
pub mod gate;
pub mod message;
pub mod routing;
pub mod status;
pub mod testing;
pub mod tracking;
pub mod traits;
pub mod workspace;

/// The engine's semantic version, surfaced in logs and handoff artifacts.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod smoke {
    #[test]
    fn version_is_present() {
        assert!(!super::VERSION.is_empty());
    }
}
