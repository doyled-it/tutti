// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure data the engine reasons over. No behavior beyond small helpers.

use serde::{Deserialize, Serialize};

/// A forge issue number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IssueId(pub u64);

/// Whether the forge considers an issue open or closed.
///
/// This is the forge's own state, NOT the `status:*` label lifecycle. The two are
/// independent: `record` only writes `status:done`, it never closes the issue, so a
/// Tutti-shipped issue stays open. Conversely an issue closed by a human, or auto-closed
/// by a `Closes #N` in a merged PR, carries no status label at all. Without this field a
/// closed-and-unlabelled issue is indistinguishable from one nobody has triaged yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    #[default]
    Open,
    Closed,
}

/// A tracked unit of work as the engine sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub milestone: Option<String>,
    /// `#[serde(default)]` (Open) so a payload written before this field existed decodes,
    /// and so the many test fixtures that predate it keep meaning "an open issue".
    #[serde(default)]
    pub state: IssueState,
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
    /// Optional milestone scope by title. `None` considers issues in any milestone
    /// (and issues with no milestone); `Some(title)` restricts to that milestone only.
    /// `#[serde(default)]` keeps configs written before this field existed parseable.
    #[serde(default)]
    pub milestone: Option<String>,
    /// Prefer the earliest open milestone, falling through to the next once that one has
    /// no ready work left, and finally to this filter unscoped. A *soft* floor: it orders
    /// where the selector looks first, it never hides work. Ignored when `milestone` is
    /// set, since an explicit scope is already a hard answer to the same question.
    #[serde(default)]
    pub milestone_floor: bool,
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
            state: IssueState::Open,
        };
        assert!(issue.has_label("status:ready"));
        assert!(!issue.has_label("status:read"));
    }
}
