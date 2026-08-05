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

/// The order the milestone floor searches in: open milestones, earliest first.
///
/// "Earliest" is by due date. Dates are ISO (`YYYY-MM-DD`) and kept opaque, so a plain
/// string compare is already chronological. Undated milestones sort last: a milestone
/// nobody has committed to a date for is not a floor anyone is standing on. `id` breaks
/// ties so the order never depends on whatever order the forge happened to list them in.
///
/// Closed milestones are dropped entirely, which is what makes the floor advance: close
/// `v0.1` and `v0.2` becomes the head of this list on the next iteration.
pub fn milestone_floor_order(milestones: &[Milestone]) -> Vec<&Milestone> {
    let mut open: Vec<&Milestone> = milestones
        .iter()
        .filter(|m| m.state == TrackState::Open)
        .collect();
    open.sort_by(|a, b| match (&a.due, &b.due) {
        (Some(x), Some(y)) => x.cmp(y).then(a.id.0.cmp(&b.id.0)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.0.cmp(&b.id.0),
    });
    open
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

    fn ms(id: u64, title: &str, state: TrackState, due: Option<&str>) -> Milestone {
        Milestone {
            id: MilestoneId(id),
            title: title.into(),
            state,
            due: due.map(str::to_string),
            progress: Progress::default(),
        }
    }

    #[test]
    fn floor_order_is_earliest_due_first_with_undated_last() {
        let all = vec![
            ms(3, "v0.3", TrackState::Open, None),
            ms(2, "v0.2", TrackState::Open, Some("2026-09-01")),
            ms(1, "v0.1", TrackState::Open, Some("2026-08-01")),
        ];
        let got: Vec<&str> = milestone_floor_order(&all)
            .iter()
            .map(|m| m.title.as_str())
            .collect();
        assert_eq!(got, ["v0.1", "v0.2", "v0.3"]);
    }

    #[test]
    fn floor_order_drops_closed_milestones() {
        // Closing the head is exactly how the floor advances to the next milestone.
        let all = vec![
            ms(1, "v0.1", TrackState::Closed, Some("2026-08-01")),
            ms(2, "v0.2", TrackState::Open, Some("2026-09-01")),
        ];
        let got = milestone_floor_order(&all);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "v0.2");
    }

    #[test]
    fn floor_order_breaks_ties_on_id_not_list_order() {
        // Same due date, and the forge listed the higher id first. Order must not depend
        // on that, or the floor would wobble between iterations.
        let all = vec![
            ms(9, "b", TrackState::Open, Some("2026-08-01")),
            ms(4, "a", TrackState::Open, Some("2026-08-01")),
            ms(7, "d", TrackState::Open, None),
            ms(5, "c", TrackState::Open, None),
        ];
        let got: Vec<&str> = milestone_floor_order(&all)
            .iter()
            .map(|m| m.title.as_str())
            .collect();
        assert_eq!(got, ["a", "b", "c", "d"]);
    }

    #[test]
    fn floor_order_is_empty_when_nothing_is_open() {
        let all = vec![ms(1, "v0.1", TrackState::Closed, None)];
        assert!(milestone_floor_order(&all).is_empty());
    }
}
