// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forge-agnostic issue status the engine speaks, and its mapping to labels.
//!
//! The engine drives an issue through `Ready -> InProgress -> Done`. Label-based
//! forges (GitHub, Gitea/Codeberg) map each `Status` to a configured label and
//! transition by adding one label and removing the other two. GitLab overrides
//! this with native work-item status (slice 3B-glab).

use serde::{Deserialize, Serialize};

/// The lifecycle position of an issue in the drain loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Available to be claimed and worked.
    Ready,
    /// Claimed and being worked by a runner.
    InProgress,
    /// Shipped.
    Done,
}

/// Maps each `Status` to a label name, for label-based forges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusLabels {
    pub ready: String,
    pub in_progress: String,
    pub done: String,
}

impl Default for StatusLabels {
    fn default() -> Self {
        Self {
            ready: "status:ready".into(),
            in_progress: "status:in-progress".into(),
            done: "status:done".into(),
        }
    }
}

/// The label to add and the labels to remove to move an issue to a `Status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelTransition {
    pub add: String,
    pub remove: Vec<String>,
}

impl StatusLabels {
    /// The label for a given status.
    pub fn label(&self, s: Status) -> &str {
        match s {
            Status::Ready => &self.ready,
            Status::InProgress => &self.in_progress,
            Status::Done => &self.done,
        }
    }

    /// The transition to `to`: add its label, remove the other two. Removing both
    /// others (not just the immediate predecessor) keeps the label set consistent
    /// regardless of the issue's prior state; a forge's remove-label call must be
    /// idempotent for a label that is absent (GitHub's `gh issue edit` is).
    pub fn transition(&self, to: Status) -> LabelTransition {
        let all = [
            (Status::Ready, &self.ready),
            (Status::InProgress, &self.in_progress),
            (Status::Done, &self.done),
        ];
        LabelTransition {
            add: self.label(to).to_string(),
            remove: all
                .iter()
                .filter(|(s, _)| *s != to)
                .map(|(_, l)| (*l).clone())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_labels_match_the_shipped_convention() {
        let l = StatusLabels::default();
        assert_eq!(l.label(Status::Ready), "status:ready");
        assert_eq!(l.label(Status::InProgress), "status:in-progress");
        assert_eq!(l.label(Status::Done), "status:done");
    }

    #[test]
    fn transition_adds_target_and_removes_the_other_two() {
        let l = StatusLabels::default();
        let t = l.transition(Status::InProgress);
        assert_eq!(t.add, "status:in-progress");
        assert_eq!(t.remove, vec!["status:ready", "status:done"]);

        let t = l.transition(Status::Done);
        assert_eq!(t.add, "status:done");
        assert_eq!(t.remove, vec!["status:ready", "status:in-progress"]);
    }

    #[test]
    fn transition_honours_custom_labels() {
        let l = StatusLabels {
            ready: "todo".into(),
            in_progress: "doing".into(),
            done: "shipped".into(),
        };
        let t = l.transition(Status::Ready);
        assert_eq!(t.add, "todo");
        assert_eq!(t.remove, vec!["doing", "shipped"]);
    }
}
