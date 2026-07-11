// SPDX-License-Identifier: AGPL-3.0-or-later
//! An in-memory Forge: issues, labels, branches, PRs, CI, all in a Mutex.

use crate::domain::{
    CiState, Issue, IssueId, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use crate::message::NewIssue;
use crate::tracking::{Epic, EpicId, Milestone, MilestoneId, Progress, Roadmap, TrackState};
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
    /// The merge mode passed to the most recent `merge` call, whatever the outcome.
    last_merge_mode: Option<MergeMode>,
    /// Tracking model.
    milestones: Vec<Milestone>,
    epics: Vec<Epic>,
    /// Which milestone an issue belongs to.
    milestone_of: HashMap<IssueId, MilestoneId>,
    /// Fresh issue-number counter for `create_issue` / `create_epic`.
    next_issue: u64,
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
        // Seed the fresh-issue counter above any preloaded id so created issues
        // never collide with the fixtures.
        let next_issue = issues.iter().map(|i| i.id.0).max().unwrap_or(0) + 1;
        Self {
            state: Mutex::new(State {
                issues,
                next_pr: 1,
                next_issue,
                ..State::default()
            }),
            default_ci: Mutex::new(default_ci),
        }
    }

    /// The state of a milestone, for assertions.
    pub fn milestone_state(&self, id: MilestoneId) -> TrackState {
        let st = self.state.lock().unwrap();
        st.milestones
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.state)
            .unwrap_or(TrackState::Open)
    }

    /// The number of issues currently tracked, for assertions.
    pub fn issue_count(&self) -> usize {
        self.state.lock().unwrap().issues.len()
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

    /// The merge mode passed to the most recent `merge` call, if any.
    pub fn last_merge_mode(&self) -> Option<MergeMode> {
        self.state.lock().unwrap().last_merge_mode
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

    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        st.last_merge_mode = Some(how);
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

    async fn list_milestones(&self) -> Result<Vec<Milestone>> {
        Ok(self.state.lock().unwrap().milestones.clone())
    }

    async fn milestone_children(&self, id: MilestoneId) -> Result<Vec<Issue>> {
        let st = self.state.lock().unwrap();
        Ok(st
            .issues
            .iter()
            .filter(|i| st.milestone_of.get(&i.id) == Some(&id))
            .cloned()
            .collect())
    }

    async fn list_epics(&self) -> Result<Vec<Epic>> {
        Ok(self.state.lock().unwrap().epics.clone())
    }

    async fn roadmap(&self) -> Result<Roadmap> {
        let st = self.state.lock().unwrap();
        Ok(Roadmap {
            milestones: st
                .milestones
                .iter()
                .filter(|m| m.state == TrackState::Open)
                .cloned()
                .collect(),
        })
    }

    async fn create_milestone(
        &self,
        title: &str,
        due: Option<&str>,
        _description: &str,
    ) -> Result<Milestone> {
        let mut st = self.state.lock().unwrap();
        let id = MilestoneId(st.milestones.len() as u64 + 1);
        let milestone = Milestone {
            id,
            title: title.to_string(),
            state: TrackState::Open,
            due: due.map(|d| d.to_string()),
            progress: Progress::default(),
        };
        st.milestones.push(milestone.clone());
        Ok(milestone)
    }

    async fn close_milestone(&self, id: MilestoneId) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        match st.milestones.iter_mut().find(|m| m.id == id) {
            Some(m) => {
                m.state = TrackState::Closed;
                Ok(())
            }
            None => Err(EngineError::Forge(format!("no such milestone {}", id.0))),
        }
    }

    async fn create_epic(&self, title: &str, _body: &str) -> Result<Epic> {
        let mut st = self.state.lock().unwrap();
        let number = st.next_issue;
        st.next_issue += 1;
        let epic = Epic {
            id: EpicId(number),
            title: title.to_string(),
            children: Vec::new(),
            progress: Progress::default(),
        };
        st.epics.push(epic.clone());
        Ok(epic)
    }

    async fn link_sub_issue(&self, parent: IssueId, child: IssueId) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if let Some(epic) = st.epics.iter_mut().find(|e| e.id.0 == parent.0) {
            if !epic.children.contains(&child) {
                epic.children.push(child);
            }
        }
        Ok(())
    }

    async fn create_issue(
        &self,
        new: &NewIssue,
        milestone: Option<MilestoneId>,
        epic: Option<EpicId>,
    ) -> Result<Issue> {
        let mut st = self.state.lock().unwrap();
        let number = st.next_issue;
        st.next_issue += 1;
        let id = IssueId(number);
        let mut labels = new.labels.clone();
        if !labels.iter().any(|l| l == "status:ready") {
            labels.push("status:ready".to_string());
        }
        let issue = Issue {
            id,
            title: new.title.clone(),
            body: new.body.clone(),
            labels,
            milestone: milestone.and_then(|m| {
                st.milestones
                    .iter()
                    .find(|ms| ms.id == m)
                    .map(|ms| ms.title.clone())
            }),
        };
        if let Some(m) = milestone {
            st.milestone_of.insert(id, m);
        }
        if let Some(e) = epic {
            if let Some(epic) = st.epics.iter_mut().find(|x| x.id == e) {
                epic.children.push(id);
            }
        }
        st.issues.push(issue.clone());
        Ok(issue)
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

    fn new_issue(title: &str) -> NewIssue {
        NewIssue {
            title: title.into(),
            body: String::new(),
            labels: vec![],
        }
    }

    #[tokio::test]
    async fn create_issue_places_under_milestone() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let milestone = forge
            .create_milestone("v0.1", None, "first release")
            .await
            .unwrap();
        let created = forge
            .create_issue(&new_issue("do a thing"), Some(milestone.id), None)
            .await
            .unwrap();
        let children = forge.milestone_children(milestone.id).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, created.id);
        assert_eq!(children[0].milestone.as_deref(), Some("v0.1"));
        assert!(created.has_label("status:ready"));
    }

    #[tokio::test]
    async fn close_milestone_flips_state() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        let milestone = forge.create_milestone("v0.1", None, "").await.unwrap();
        assert_eq!(forge.milestone_state(milestone.id), TrackState::Open);
        forge.close_milestone(milestone.id).await.unwrap();
        assert_eq!(forge.milestone_state(milestone.id), TrackState::Closed);
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
