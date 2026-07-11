// SPDX-License-Identifier: AGPL-3.0-or-later
//! An in-memory Forge: issues, labels, branches, PRs, CI, all in a Mutex.

use crate::domain::{
    CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use crate::traits::{ClaimGuard, EngineError, Forge, Result};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Default)]
struct State {
    issues: Vec<Issue>,
    branches: HashSet<String>,
    prs: HashMap<u64, PrHandle>,
    /// PR number -> its base branch (the integration target it merges into).
    bases: HashMap<u64, String>,
    ci: HashMap<u64, CiState>,
    next_pr: u64,
    done: HashSet<IssueId>,
    records: Vec<(IssueId, ShipRecord)>,
}

/// A scriptable in-memory forge. Configure CI outcomes per-branch via `set_ci_for_next_pr`.
pub struct FakeForge {
    state: Mutex<State>,
    /// CI verdict every newly opened PR receives.
    default_ci: Mutex<CiState>,
}

impl FakeForge {
    /// Build a forge preloaded with `issues`; every new PR gets `default_ci`.
    pub fn new(issues: Vec<Issue>, default_ci: CiState) -> Self {
        Self {
            state: Mutex::new(State {
                issues,
                next_pr: 1,
                ..State::default()
            }),
            default_ci: Mutex::new(default_ci),
        }
    }

    pub fn labels_of(&self, issue: IssueId) -> Vec<String> {
        let st = self.state.lock().unwrap();
        st.issues
            .iter()
            .find(|i| i.id == issue)
            .map(|i| i.labels.clone())
            .unwrap_or_default()
    }

    pub fn is_done(&self, issue: IssueId) -> bool {
        self.state.lock().unwrap().done.contains(&issue)
    }

    /// The base branches merged into (one per recorded ship), read from the PR bases.
    pub fn merged_bases(&self) -> Vec<String> {
        let st = self.state.lock().unwrap();
        st.records
            .iter()
            .filter_map(|(_, r)| st.bases.get(&r.pr.number).cloned())
            .collect()
    }

    fn set_labels(st: &mut State, issue: IssueId, add: &str, remove: &str) {
        if let Some(i) = st.issues.iter_mut().find(|i| i.id == issue) {
            i.labels.retain(|l| l != remove);
            if !i.labels.iter().any(|l| l == add) {
                i.labels.push(add.to_string());
            }
        }
    }
}

#[async_trait]
impl Forge for FakeForge {
    async fn next_ready_issue(&self, filter: &SelectFilter) -> Result<Option<Issue>> {
        let st = self.state.lock().unwrap();
        Ok(st
            .issues
            .iter()
            .find(|i| {
                i.has_label(&filter.require_label)
                    && !filter.skip_labels.iter().any(|s| i.has_label(s))
            })
            .cloned())
    }

    async fn claim(&self, issue: IssueId) -> Result<ClaimGuard> {
        let mut st = self.state.lock().unwrap();
        let i = st.issues.iter().find(|i| i.id == issue).cloned();
        match i {
            Some(i) if i.has_label("status:in-progress") => Err(EngineError::Forge(format!(
                "issue {} already claimed",
                issue.0
            ))),
            Some(_) => {
                Self::set_labels(&mut st, issue, "status:in-progress", "status:ready");
                Ok(ClaimGuard::new(issue))
            }
            None => Err(EngineError::Forge(format!("no such issue {}", issue.0))),
        }
    }

    async fn release(&self, issue: IssueId) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        Self::set_labels(&mut st, issue, "status:ready", "status:in-progress");
        Ok(())
    }

    async fn branch_exists(&self, branch: &str) -> Result<bool> {
        Ok(self.state.lock().unwrap().branches.contains(branch))
    }

    async fn create_branch(&self, branch: &str, _from: &str) -> Result<()> {
        self.state
            .lock()
            .unwrap()
            .branches
            .insert(branch.to_string());
        Ok(())
    }

    async fn push_branch(&self, branch: &str) -> Result<()> {
        // Record it as an existing branch so downstream PR opens stay valid.
        self.state
            .lock()
            .unwrap()
            .branches
            .insert(branch.to_string());
        Ok(())
    }

    async fn open_pr(&self, pr: PrRequest) -> Result<PrHandle> {
        let mut st = self.state.lock().unwrap();
        let number = st.next_pr;
        st.next_pr += 1;
        let handle = PrHandle {
            number,
            branch: pr.head.clone(),
        };
        st.prs.insert(number, handle.clone());
        st.bases.insert(number, pr.base.clone());
        let verdict = *self.default_ci.lock().unwrap();
        st.ci.insert(number, verdict);
        Ok(handle)
    }

    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState> {
        Ok(*self
            .state
            .lock()
            .unwrap()
            .ci
            .get(&pr.number)
            .unwrap_or(&CiState::Pending))
    }

    async fn merge(&self, pr: &PrHandle, _how: MergeMode) -> Result<()> {
        let st = self.state.lock().unwrap();
        match st.ci.get(&pr.number) {
            Some(CiState::Pass) => Ok(()),
            other => Err(EngineError::Forge(format!("refuse merge, CI={:?}", other))),
        }
    }

    async fn record(&self, issue: IssueId, outcome: &ShipRecord) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        Self::set_labels(&mut st, issue, "status:done", "status:in-progress");
        st.done.insert(issue);
        st.records.push((issue, outcome.clone()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(id: u64) -> Issue {
        Issue {
            id: IssueId(id),
            title: format!("issue {id}"),
            body: String::new(),
            labels: vec!["status:ready".into()],
            milestone: None,
        }
    }

    fn filter() -> SelectFilter {
        SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
        }
    }

    #[tokio::test]
    async fn claim_flips_labels_and_blocks_double_claim() {
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let _g = forge.claim(IssueId(1)).await.unwrap();
        assert!(forge
            .labels_of(IssueId(1))
            .contains(&"status:in-progress".to_string()));
        assert!(forge.claim(IssueId(1)).await.is_err());
    }

    #[tokio::test]
    async fn selector_skips_needs_human() {
        let mut nh = ready(2);
        nh.labels.push("status:needs-human".into());
        let forge = FakeForge::new(vec![nh], CiState::Pass);
        assert!(forge.next_ready_issue(&filter()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn merge_refused_unless_ci_pass() {
        let forge = FakeForge::new(vec![ready(3)], CiState::Fail);
        let pr = forge
            .open_pr(PrRequest {
                base: "version/v0.1".into(),
                head: "feat/x-3".into(),
                title: "t".into(),
                body: "b".into(),
                labels: vec![],
            })
            .await
            .unwrap();
        assert!(forge.merge(&pr, MergeMode::Squash).await.is_err());
    }
}
