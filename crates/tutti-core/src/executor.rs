// SPDX-License-Identifier: AGPL-3.0-or-later
//! The mechanical ship path. Total over the state machine: every branch either
//! ships or stops clean, never half-finishes.

use crate::domain::{CiState, MergeMode, PrHandle, PrRequest, ShipRecord};
use crate::message::Handoff;
use crate::traits::{EngineError, Forge, Result};

/// The mechanical ship path. Polls CI up to `ci_max_polls` times, sleeping
/// `poll_delay` between polls, then merges or leaves the PR open.
pub struct Executor<'a> {
    pub forge: &'a dyn Forge,
    /// The protected trunk. GUARDRAIL: the executor refuses to merge here.
    pub trunk: String,
    /// How many times to poll CI before giving up.
    pub ci_max_polls: u32,
    /// How long to wait between CI polls.
    pub poll_delay: std::time::Duration,
}

/// Outcome of a ship attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum ShipResult {
    Merged(PrHandle),
    /// CI never passed; PR left open for a human.
    CiNotGreen(PrHandle, CiState),
}

impl<'a> Executor<'a> {
    /// Ship the work described by `handoff`. Returns Merged on success.
    pub async fn ship(&self, handoff: &Handoff) -> Result<ShipResult> {
        // GUARDRAIL #2: never merge into the protected trunk.
        if handoff.target.target == self.trunk {
            return Err(EngineError::Guardrail(format!(
                "routing tried to target the protected trunk '{}'",
                self.trunk
            )));
        }

        // Ensure the integration branch exists.
        if !self.forge.branch_exists(&handoff.target.target).await? {
            let from = handoff.target.create_from.as_deref().unwrap_or(&self.trunk);
            self.forge
                .create_branch(&handoff.target.target, from)
                .await?;
        }

        let pr = self
            .forge
            .open_pr(PrRequest {
                base: handoff.target.target.clone(),
                head: handoff.branch.clone(),
                title: handoff.pr_title.clone(),
                body: handoff.pr_body.clone(),
                labels: handoff.labels.clone(),
            })
            .await?;

        // Poll CI.
        let mut last = CiState::Pending;
        for _ in 0..self.ci_max_polls {
            last = self.forge.ci_status(&pr).await?;
            match last {
                CiState::Pass => break,
                CiState::Fail => return Ok(ShipResult::CiNotGreen(pr, CiState::Fail)),
                CiState::Pending => {
                    tokio::time::sleep(self.poll_delay).await;
                    continue;
                }
            }
        }
        if last != CiState::Pass {
            return Ok(ShipResult::CiNotGreen(pr, last));
        }

        self.forge.merge(&pr, MergeMode::Squash).await?;
        self.forge
            .record(
                handoff.issue,
                &ShipRecord {
                    pr: pr.clone(),
                    decision_note: handoff.decision_note.clone(),
                },
            )
            .await?;
        Ok(ShipResult::Merged(pr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{BranchPlan, Issue, IssueId};
    use crate::testing::FakeForge;

    fn handoff_to(target: &str) -> Handoff {
        Handoff {
            issue: IssueId(1),
            branch: "feat/x-1".into(),
            target: BranchPlan {
                target: target.into(),
                create_from: Some("main".into()),
            },
            pr_title: "t".into(),
            pr_body: "b".into(),
            labels: vec![],
            decision_note: Some("chose X".into()),
        }
    }

    fn ready() -> Issue {
        Issue {
            id: IssueId(1),
            title: "t".into(),
            body: String::new(),
            labels: vec!["status:in-progress".into()],
            milestone: None,
        }
    }

    #[tokio::test]
    async fn merges_on_green_and_records() {
        let forge = FakeForge::new(vec![ready()], CiState::Pass);
        let exec = Executor {
            forge: &forge,
            trunk: "main".into(),
            ci_max_polls: 3,
            poll_delay: std::time::Duration::from_millis(0),
        };
        let res = exec.ship(&handoff_to("version/v0.1")).await.unwrap();
        assert!(matches!(res, ShipResult::Merged(_)));
        assert!(forge.is_done(IssueId(1)));
    }

    #[tokio::test]
    async fn leaves_pr_open_on_red_ci() {
        let forge = FakeForge::new(vec![ready()], CiState::Fail);
        let exec = Executor {
            forge: &forge,
            trunk: "main".into(),
            ci_max_polls: 3,
            poll_delay: std::time::Duration::from_millis(0),
        };
        let res = exec.ship(&handoff_to("version/v0.1")).await.unwrap();
        assert!(matches!(res, ShipResult::CiNotGreen(_, CiState::Fail)));
        assert!(!forge.is_done(IssueId(1)));
    }

    #[tokio::test]
    async fn guardrail_refuses_trunk_target() {
        let forge = FakeForge::new(vec![ready()], CiState::Pass);
        let exec = Executor {
            forge: &forge,
            trunk: "main".into(),
            ci_max_polls: 3,
            poll_delay: std::time::Duration::from_millis(0),
        };
        let err = exec.ship(&handoff_to("main")).await.unwrap_err();
        assert!(matches!(err, EngineError::Guardrail(_)));
    }
}
