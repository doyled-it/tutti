// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure data the engine reasons over. No behavior beyond small helpers.

use serde::{Deserialize, Serialize};

/// A forge issue number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueId(pub u64);

/// A tracked unit of work as the engine sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub milestone: Option<String>,
}

impl Issue {
    /// True when the issue carries `label`.
    pub fn has_label(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }
}

/// Which issues the selector will consider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectFilter {
    pub require_label: String,
    pub skip_labels: Vec<String>,
}

/// Where an issue's work merges, and (if the branch is new) what to branch it from.
/// `target` is NEVER the trunk; the executor enforces that invariant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchPlan {
    pub target: String,
    pub create_from: Option<String>,
}

/// A pull/merge request the engine has opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrHandle {
    pub number: u64,
    /// The HEAD branch of the PR (the feature branch), not its base.
    pub branch: String,
}

/// A request to open a PR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRequest {
    pub base: String,
    pub head: String,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// CI state on a PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CiState {
    Pending,
    Pass,
    Fail,
}

/// How to merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeMode {
    Squash,
    Merge,
    Rebase,
}

/// What the engine records once an issue ships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipRecord {
    pub pr: PrHandle,
    pub decision_note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_label_matches_exactly() {
        let issue = Issue {
            id: IssueId(7),
            title: "t".into(),
            body: "b".into(),
            labels: vec!["status:ready".into()],
            milestone: None,
        };
        assert!(issue.has_label("status:ready"));
        assert!(!issue.has_label("status:read"));
    }
}
