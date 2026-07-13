// SPDX-License-Identifier: AGPL-3.0-or-later
//! The tracking hierarchy above a single issue: milestones, epics, roadmap.

use crate::domain::IssueId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MilestoneId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EpicId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackState {
    Open,
    Closed,
}

/// Completion rollup for a milestone or epic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub total: u32,
    pub done: u32,
}

impl Progress {
    /// True when there is at least one child and all children are done.
    pub fn is_drained(&self) -> bool {
        self.total > 0 && self.done == self.total
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    pub id: MilestoneId,
    pub title: String,
    pub state: TrackState,
    pub due: Option<String>, // ISO date; kept opaque
    pub progress: Progress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Epic {
    pub id: EpicId,
    pub title: String,
    pub children: Vec<IssueId>,
    pub progress: Progress,
}

/// A read-only, derived view for the future UI: open milestones ordered by due date.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roadmap {
    pub milestones: Vec<Milestone>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drained_requires_children_and_all_done() {
        assert!(!Progress { total: 0, done: 0 }.is_drained());
        assert!(!Progress { total: 3, done: 2 }.is_drained());
        assert!(Progress { total: 3, done: 3 }.is_drained());
    }
}
