// SPDX-License-Identifier: AGPL-3.0-or-later
//! A backend that returns pre-scripted outcomes keyed by role.

use crate::message::{AgentEvent, AgentOutcome, AgentTask, Role};
use crate::traits::{AgentBackend, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::mpsc::Sender;

/// Maps each role to the outcome it should return. Missing roles error.
pub struct FakeBackend {
    scripted: Mutex<HashMap<Role, Vec<AgentOutcome>>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self {
            scripted: Mutex::new(HashMap::new()),
        }
    }

    /// Queue `outcome` to be returned on the next run of `role` (FIFO).
    pub fn script(mut self, role: Role, outcome: AgentOutcome) -> Self {
        self.scripted
            .get_mut()
            .unwrap()
            .entry(role)
            .or_default()
            .push(outcome);
        self
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentBackend for FakeBackend {
    async fn run(
        &self,
        task: AgentTask,
        _worktree: &Path,
        events: Sender<AgentEvent>,
    ) -> Result<AgentOutcome> {
        let _ = events
            .send(AgentEvent::Line(format!("fake {:?}", task.playbook.role)))
            .await;
        let _ = events.send(AgentEvent::Done).await;
        let mut map = self.scripted.lock().unwrap();
        let queue = map.get_mut(&task.playbook.role);
        match queue {
            Some(q) if !q.is_empty() => Ok(q.remove(0)),
            _ => Err(crate::traits::EngineError::Backend(format!(
                "no scripted outcome for {:?}",
                task.playbook.role
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{BranchPlan, Issue, IssueId};
    use crate::message::*;

    #[tokio::test]
    async fn returns_scripted_outcome_for_role() {
        let outcome = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: Some(Handoff {
                issue: IssueId(1),
                branch: "feat/x-1".into(),
                target: BranchPlan {
                    target: "version/v0.1".into(),
                    create_from: None,
                },
                pr_title: "t".into(),
                pr_body: "b".into(),
                labels: vec![],
                decision_note: None,
            }),
            review: None,
            summary: "done".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        let backend = FakeBackend::new().script(Role::Implementer, outcome);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let task = AgentTask {
            playbook: RolePlaybook {
                role: Role::Implementer,
                skills: vec![],
            },
            issue: Issue {
                id: IssueId(1),
                title: "t".into(),
                body: "b".into(),
                labels: vec![],
                milestone: None,
            },
            worktree_branch: "feat/x-1".into(),
            model: "fake".into(),
            review: None,
        };
        let got = backend.run(task, Path::new("."), tx).await.unwrap();
        assert_eq!(got.status, AgentStatus::ReadyToShip);
    }
}
