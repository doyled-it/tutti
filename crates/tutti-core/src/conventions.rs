// SPDX-License-Identifier: AGPL-3.0-or-later
//! The seam by which the engine injects per-language convention text into a role's prompt.
//! The implementation lives in a higher crate that can reach the skill files and language
//! detection; the engine holds only this trait, mirroring `context::ContextProvider`.

use crate::message::Role;
use std::path::Path;

pub trait ConventionsProvider: Send + Sync {
    /// The convention preamble to inject for `role` working in `worktree`, or `None` when
    /// there is nothing to inject (an unsupported role, or no detected language with a
    /// reference).
    fn preamble_for(&self, role: Role, worktree: &Path) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;
    use std::path::Path;

    struct FakeProvider;
    impl ConventionsProvider for FakeProvider {
        fn preamble_for(&self, role: Role, _worktree: &Path) -> Option<String> {
            match role {
                Role::Implementer | Role::Reviewer | Role::FixApplier => {
                    Some("CONVENTIONS".to_string())
                }
                _ => None,
            }
        }
    }

    #[test]
    fn provider_gives_a_preamble_only_to_the_coding_roles() {
        let p = FakeProvider;
        assert_eq!(
            p.preamble_for(Role::Implementer, Path::new("/x")),
            Some("CONVENTIONS".to_string())
        );
        assert!(p.preamble_for(Role::Planner, Path::new("/x")).is_none());
    }
}
