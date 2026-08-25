// SPDX-License-Identifier: AGPL-3.0-or-later
//! How a project's shape selects and sizes the design rails.

use serde::{Deserialize, Serialize};

/// The shape of the project being designed. Selects which movements run (and, later,
/// their depth). See the spec's branching table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectShape {
    /// A CLI tool or single library. Collapses the middle of the chain.
    SmallCli,
    /// A mobile app. Keeps the full chain; goes light on domain/structure (depth is E5).
    Mobile,
    /// A multi-service product. Runs the full chain at full depth.
    MultiService,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json_snake_case() {
        let json = serde_json::to_string(&ProjectShape::MultiService).unwrap();
        assert_eq!(json, "\"multi_service\"");
        let back: ProjectShape = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ProjectShape::MultiService);
    }
}
