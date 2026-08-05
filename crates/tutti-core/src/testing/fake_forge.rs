// SPDX-License-Identifier: AGPL-3.0-or-later
//! An in-memory Forge: issues, labels, branches, PRs, CI, all in a Mutex.

use crate::domain::{
    CiState, Issue, IssueId, IssueState, MergeMode, PrHandle, PrRequest, SelectFilter, ShipRecord,
};
use crate::message::NewIssue;
use crate::status::{Status, StatusLabels};
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
    /// Issues whose `edit_labels` is scripted to fail (see `fail_label_edits_for`).
    label_edit_failures: HashSet<IssueId>,
    /// How many times each Forge read has been called, so a caller's COST claims can be
    /// asserted rather than assumed. Without this, a refactor that reintroduces a fetch per
    /// candidate ordering passes every behavioural test.
    calls: HashMap<&'static str, usize>,
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

    /// The titles of all tracked issues, for assertions.
    pub fn issue_titles(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .issues
            .iter()
            .map(|i| i.title.clone())
            .collect()
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

    /// Apply a status transition using the default label mapping: add the target
    /// label, remove the other two.
    fn apply_status(st: &mut State, issue: IssueId, to: Status) {
        let t = StatusLabels::default().transition(to);
        Self::apply_labels(st, issue, std::slice::from_ref(&t.add), &t.remove);
    }

    /// The shared label write: remove first, then add, skipping duplicates. Removing an
    /// absent label is a no-op, matching every real adapter.
    fn apply_labels(st: &mut State, issue: IssueId, add: &[String], remove: &[String]) {
        if let Some(i) = st.issues.iter_mut().find(|i| i.id == issue) {
            i.labels.retain(|l| !remove.contains(l));
            for a in add {
                if !i.labels.contains(a) {
                    i.labels.push(a.clone());
                }
            }
        }
    }

    /// How many times `method` has been called on this forge.
    pub fn call_count(&self, method: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .get(method)
            .copied()
            .unwrap_or(0)
    }

    /// Make `edit_labels` fail for `issue`, so a caller's partial-failure accounting can be
    /// tested. Only `edit_labels` honours this; the status writes are left alone so existing
    /// engine tests are unaffected.
    pub fn fail_label_edits_for(&self, issue: IssueId) {
        self.state.lock().unwrap().label_edit_failures.insert(issue);
    }
}

#[async_trait]
impl Forge for FakeForge {
    async fn list_ready_issues(&self, filter: &SelectFilter) -> Result<Vec<Issue>> {
        let mut st = self.state.lock().unwrap();
        *st.calls.entry("list_ready_issues").or_default() += 1;
        Ok(st
            .issues
            .iter()
            .filter(|i| {
                i.has_label(&filter.require_label)
                    && !filter.skip_labels.iter().any(|s| i.has_label(s))
                    && filter
                        .milestone
                        .as_ref()
                        .is_none_or(|m| i.milestone.as_ref() == Some(m))
            })
            .cloned()
            .collect())
    }

    async fn list_issues(&self) -> Result<Vec<Issue>> {
        Ok(self.state.lock().unwrap().issues.clone())
    }

    async fn list_labels(&self) -> Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }

    async fn create_label(&self, _name: &str, _color: &str) -> Result<()> {
        Ok(())
    }

    async fn edit_labels(&self, issue: IssueId, add: &[String], remove: &[String]) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        if st.label_edit_failures.contains(&issue) {
            return Err(EngineError::Forge(format!(
                "scripted label-edit failure for issue {}",
                issue.0
            )));
        }
        if !st.issues.iter().any(|i| i.id == issue) {
            return Err(EngineError::Forge(format!("no such issue {}", issue.0)));
        }
        Self::apply_labels(&mut st, issue, add, remove);
        Ok(())
    }

    async fn claim(&self, issue: IssueId) -> Result<ClaimGuard> {
        let mut st = self.state.lock().unwrap();
        let i = st.issues.iter().find(|i| i.id == issue).cloned();
        match i {
            Some(i) if i.has_label(&StatusLabels::default().in_progress) => Err(
                EngineError::Forge(format!("issue {} already claimed", issue.0)),
            ),
            Some(_) => {
                Self::apply_status(&mut st, issue, Status::InProgress);
                Ok(ClaimGuard::new(issue))
            }
            None => Err(EngineError::Forge(format!("no such issue {}", issue.0))),
        }
    }

    async fn release(&self, issue: IssueId) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        Self::apply_status(&mut st, issue, Status::Ready);
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
        Self::apply_status(&mut st, issue, Status::Done);
        st.done.insert(issue);
        st.records.push((issue, outcome.clone()));
        Ok(())
    }

    async fn list_milestones(&self) -> Result<Vec<Milestone>> {
        let mut st = self.state.lock().unwrap();
        *st.calls.entry("list_milestones").or_default() += 1;
        Ok(st.milestones.clone())
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
        let mut st = self.state.lock().unwrap();
        *st.calls.entry("list_epics").or_default() += 1;
        Ok(st.epics.clone())
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
        let ready = StatusLabels::default().ready;
        if !labels.contains(&ready) {
            labels.push(ready);
        }
        let issue = Issue {
            id,
            title: new.title.clone(),
            body: new.body.clone(),
            labels,
            state: IssueState::Open,
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
            state: IssueState::Open,
        }
    }

    fn filter() -> SelectFilter {
        SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
            milestone: None,
            milestone_floor: false,
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
    async fn list_issues_returns_all_seeded_issues() {
        let forge = FakeForge::new(vec![ready(1), ready(2), ready(3)], CiState::Pass);
        let all = forge.list_issues().await.unwrap();
        assert_eq!(all.len(), 3);
        let ids: Vec<u64> = all.iter().map(|i| i.id.0).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
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
            milestone: None,
            epic: None,
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
    async fn edit_labels_adds_and_removes() {
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        forge
            .edit_labels(
                IssueId(1),
                &["status:needs-human".into()],
                &["status:ready".into()],
            )
            .await
            .unwrap();
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
        assert!(!labels.contains(&"status:ready".to_string()));
    }

    #[tokio::test]
    async fn edit_labels_removing_an_absent_label_is_a_no_op() {
        // Callers express transitions without first reading the label set, so removing a
        // label the issue never carried must succeed.
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        forge
            .edit_labels(IssueId(1), &[], &["status:done".into()])
            .await
            .unwrap();
        assert_eq!(
            forge.labels_of(IssueId(1)),
            vec!["status:ready".to_string()]
        );
    }

    #[tokio::test]
    async fn edit_labels_does_not_duplicate_a_label_already_present() {
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        forge
            .edit_labels(IssueId(1), &["status:ready".into()], &[])
            .await
            .unwrap();
        assert_eq!(
            forge.labels_of(IssueId(1)),
            vec!["status:ready".to_string()]
        );
    }

    #[tokio::test]
    async fn edit_labels_errors_on_an_unknown_issue() {
        let forge = FakeForge::new(vec![], CiState::Pass);
        assert!(forge
            .edit_labels(IssueId(99), &["x".into()], &[])
            .await
            .is_err());
    }

    #[tokio::test]
    async fn scripted_label_edit_failure_is_surfaced() {
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        forge.fail_label_edits_for(IssueId(1));
        assert!(forge
            .edit_labels(IssueId(1), &["x".into()], &[])
            .await
            .is_err());
        // The failure must not have partially applied.
        assert_eq!(
            forge.labels_of(IssueId(1)),
            vec!["status:ready".to_string()]
        );
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
