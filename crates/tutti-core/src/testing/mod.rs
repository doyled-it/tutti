// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory fakes so the engine runs end-to-end with no side effects.

pub mod fake_backend;
pub mod fake_forge;

pub use crate::workspace::NoopWorkspace;
pub use fake_backend::FakeBackend;
pub use fake_forge::FakeForge;
