// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pluggable routing strategies. Each decides only where an issue's work merges.

pub mod phase_stacking;
pub mod trunk;

use crate::traits::RoutingStrategy;
pub use phase_stacking::PhaseStacking;
pub use trunk::Trunk;

/// Build a strategy by config name. Unknown names are an error the caller surfaces.
pub fn by_name(
    name: &str,
    integration_branch: &str,
    trunk: &str,
) -> Option<Box<dyn RoutingStrategy>> {
    match name {
        "trunk" => Some(Box::new(Trunk::new(integration_branch, trunk))),
        "phase_stacking" => Some(Box::new(PhaseStacking::new(trunk.to_string()))),
        _ => None,
    }
}
