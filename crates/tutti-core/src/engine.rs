// SPDX-License-Identifier: AGPL-3.0-or-later
//! The drain loop. One issue per iteration: select, implement, review,
//! apply-fixes, gate, merge (via the executor), record, plan.

use crate::config::Config;
use crate::domain::Issue;
use crate::executor::{Executor, ShipResult};
use crate::message::{
    AgentEvent, AgentOutcome, AgentStatus, AgentTask, PlanAction, PlanDecision, ReviewReport, Role,
    RolePlaybook,
};
use crate::routing;
use crate::traits::{AgentBackend, ClaimGuard, EngineError, Forge, Result, RoutingStrategy};
use std::path::PathBuf;

/// What one iteration of the loop produced. Drives the outer drain decision.
#[derive(Debug, PartialEq, Eq)]
pub enum IterOutcome {
    Shipped,
    NoReadyWork,
    Blocked(String),
    StoppedCiRed,
    StoppedGateRed,
}

pub struct Engine<'a> {
    pub cfg: &'a Config,
    pub forge: &'a dyn Forge,
    pub backend: &'a dyn AgentBackend,
    pub routing: Box<dyn RoutingStrategy>,
    /// Where worktrees live on disk (real backends use it; fakes ignore it).
    pub workdir: PathBuf,
}

impl<'a> Engine<'a> {
    pub fn new(
        cfg: &'a Config,
        forge: &'a dyn Forge,
        backend: &'a dyn AgentBackend,
        workdir: PathBuf,
    ) -> Result<Self> {
        let routing = routing::by_name(&cfg.routing, &cfg.integration_branch, &cfg.trunk)
            .ok_or_else(|| EngineError::Routing(format!("unknown routing '{}'", cfg.routing)))?;
        Ok(Self {
            cfg,
            forge,
            backend,
            routing,
            workdir,
        })
    }

    fn playbook(&self, role: Role) -> RolePlaybook {
        RolePlaybook {
            role,
            skills: self.cfg.skills_for(role),
        }
    }

    async fn run_role(
        &self,
        role: Role,
        issue: &Issue,
        review: Option<ReviewReport>,
    ) -> Result<AgentOutcome> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
        let task = AgentTask {
            playbook: self.playbook(role),
            issue: issue.clone(),
            worktree_branch: format!("feat/issue-{}", issue.id.0),
            model: self.cfg.model.clone(),
            review,
        };
        // Drain events into logs so the channel never blocks.
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let out = self.backend.run(task, &self.workdir, tx).await;
        let _ = drain.await;
        out
    }

    /// Run one issue end-to-end. Returns the iteration outcome. Never merges to trunk.
    pub async fn run_one(&self) -> Result<IterOutcome> {
        // GUARDRAIL #1: selection skips needs-human etc. via SelectFilter.
        let Some(issue) = self.forge.next_ready_issue(&self.cfg.select).await? else {
            return Ok(IterOutcome::NoReadyWork);
        };
        let mut guard = self.forge.claim(issue.id).await?;
        let result = self.run_claimed(&issue, &mut guard).await;
        if result.is_err() {
            // Best-effort release so a transient failure does not strand the claim.
            let _ = self.forge.release(issue.id).await;
        }
        result
    }

    /// The claimed body of one iteration. On every terminal return it disarms the
    /// guard; any unexpected `Err` propagates and `run_one` releases the claim.
    async fn run_claimed(&self, issue: &Issue, guard: &mut ClaimGuard) -> Result<IterOutcome> {
        // Stage: implement.
        let impl_out = self.run_role(Role::Implementer, issue, None).await?;
        if impl_out.status != AgentStatus::ReadyToShip {
            self.forge.release(issue.id).await?;
            guard.disarm();
            return Ok(IterOutcome::Blocked(
                impl_out.blocked_reason.unwrap_or_default(),
            ));
        }
        let Some(mut handoff) = impl_out.handoff else {
            self.forge.release(issue.id).await?;
            guard.disarm();
            return Ok(IterOutcome::Blocked(
                "agent reported ReadyToShip but produced no handoff".into(),
            ));
        };

        // Stage: review (fresh agent).
        let review_out = self.run_role(Role::Reviewer, issue, None).await?;
        let report = review_out.review.unwrap_or(ReviewReport {
            findings: vec![],
            verdict: crate::message::Verdict::Approve,
        });

        // Stage: apply-fixes if the review demands it.
        if report.needs_fixes() {
            let fix_out = self.run_role(Role::FixApplier, issue, Some(report)).await?;
            if fix_out.status != AgentStatus::ReadyToShip {
                self.forge.release(issue.id).await?;
                guard.disarm();
                return Ok(IterOutcome::Blocked(
                    fix_out.blocked_reason.unwrap_or_default(),
                ));
            }
            let Some(fix_handoff) = fix_out.handoff else {
                self.forge.release(issue.id).await?;
                guard.disarm();
                return Ok(IterOutcome::Blocked(
                    "fix applier reported ReadyToShip but produced no handoff".into(),
                ));
            };
            handoff = fix_handoff;
        }

        // The routing strategy decides the target; overwrite whatever the agent guessed.
        handoff.target = self.routing.target_branch(issue)?;

        // Stage: merge (mechanical executor). CI is the gate for real forges;
        // the local `Gate` is run by the implement stage's own tooling before handoff.
        let exec = Executor {
            forge: self.forge,
            trunk: self.cfg.trunk.clone(),
            ci_max_polls: self.cfg.ci_max_polls,
            poll_delay: std::time::Duration::from_secs(self.cfg.poll_delay_secs),
        };
        match exec.ship(&handoff).await? {
            ShipResult::Merged(_) => {
                guard.disarm(); // record() already flipped the label to done
                Ok(IterOutcome::Shipped)
            }
            ShipResult::CiNotGreen(_, _) => {
                // Leave the issue in-progress and the PR open for a human.
                guard.disarm();
                Ok(IterOutcome::StoppedCiRed)
            }
        }
    }

    /// Drain up to `max_issues_per_run` issues, then run the planning hook once.
    pub async fn drain(&self) -> Result<(u32, Option<PlanDecision>)> {
        let mut shipped = 0;
        for _ in 0..self.cfg.max_issues_per_run {
            match self.run_one().await? {
                IterOutcome::Shipped => shipped += 1,
                IterOutcome::NoReadyWork => break,
                // Any stop condition halts the drain so a human can look.
                _ => break,
            }
        }
        let plan = if shipped > 0 {
            Some(self.plan().await?)
        } else {
            None
        };
        Ok((shipped, plan))
    }

    /// The planning hook. GUARDRAIL #3: only whitelisted, non-human actions execute;
    /// everything else is returned for a human to action.
    async fn plan(&self) -> Result<PlanDecision> {
        // A real planner is an agent run; slice-1 default is deterministic "NextIssue".
        // (The Planner role + agent wiring lands with the live backend.)
        Ok(PlanDecision {
            action: PlanAction::NextIssue,
            rationale: "drain next ready issue".into(),
            needs_human: false,
        })
    }
}

/// GUARDRAIL #3 helper: is this plan action safe to auto-execute?
pub fn plan_is_auto_executable(decision: &PlanDecision) -> bool {
    if decision.needs_human {
        return false;
    }
    matches!(
        decision.action,
        PlanAction::NextIssue | PlanAction::CreateIssues(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::domain::{BranchPlan, CiState, Issue, IssueId, SelectFilter};
    use crate::gate::Gate;
    use crate::message::*;
    use crate::testing::{FakeBackend, FakeForge};

    fn cfg() -> Config {
        Config {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "version/v0.1".into(),
            model: "fake".into(),
            max_issues_per_run: 5,
            ci_max_polls: 40,
            poll_delay_secs: 0,
            select: SelectFilter {
                require_label: "status:ready".into(),
                skip_labels: vec!["status:needs-human".into()],
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
            roles: crate::config::default_roles(),
        }
    }

    fn ready(id: u64) -> Issue {
        Issue {
            id: IssueId(id),
            title: format!("i{id}"),
            body: String::new(),
            labels: vec!["status:ready".into()],
            milestone: None,
        }
    }

    fn ship_outcome(id: u64) -> AgentOutcome {
        AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: Some(Handoff {
                issue: IssueId(id),
                branch: format!("feat/issue-{id}"),
                target: BranchPlan {
                    target: "IGNORED".into(),
                    create_from: None,
                },
                pr_title: "t".into(),
                pr_body: "b".into(),
                labels: vec![],
                decision_note: None,
            }),
            review: None,
            summary: "ok".into(),
            usage: Usage::default(),
            blocked_reason: None,
        }
    }

    fn clean_review() -> AgentOutcome {
        AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![],
                verdict: Verdict::Approve,
            }),
            summary: "lgtm".into(),
            usage: Usage::default(),
            blocked_reason: None,
        }
    }

    #[tokio::test]
    async fn happy_path_ships_one_issue_to_integration_branch() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::Shipped);
        assert!(forge.is_done(IssueId(1)));
        // The routing strategy, not the agent's guess, decided the branch.
        assert!(forge.merged_bases().contains(&"version/v0.1".to_string()));
    }

    #[tokio::test]
    async fn review_requesting_changes_triggers_fix_stage() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let dirty_review = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Blocking,
                    file: "a.rs".into(),
                    line: None,
                    claim: "bug".into(),
                }],
                verdict: Verdict::RequestChanges,
            }),
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, dirty_review)
            .script(Role::FixApplier, ship_outcome(1));
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();
        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::Shipped);
    }

    #[tokio::test]
    async fn blocked_implementer_releases_the_claim() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let blocked = AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            summary: "needs hardware".into(),
            usage: Usage::default(),
            blocked_reason: Some("needs a device".into()),
        };
        let backend = FakeBackend::new().script(Role::Implementer, blocked);
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();
        assert!(matches!(
            engine.run_one().await.unwrap(),
            IterOutcome::Blocked(_)
        ));
        // Released back to ready for the next run.
        assert!(forge
            .labels_of(IssueId(1))
            .contains(&"status:ready".to_string()));
    }

    #[tokio::test]
    async fn no_ready_work_is_reported() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![], CiState::Pass);
        let backend = FakeBackend::new();
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();
        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::NoReadyWork);
    }

    #[tokio::test]
    async fn ci_red_leaves_issue_in_progress() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Fail);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::StoppedCiRed);
        // CI red must not release the claim: the issue stays in-progress, not done,
        // and not handed back to the ready pool.
        assert!(!forge.is_done(IssueId(1)));
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:in-progress".to_string()));
        assert!(!labels.contains(&"status:ready".to_string()));
    }

    #[tokio::test]
    async fn fix_applier_blocked_releases_claim() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let dirty_review = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Blocking,
                    file: "a.rs".into(),
                    line: None,
                    claim: "bug".into(),
                }],
                verdict: Verdict::RequestChanges,
            }),
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        let fix_blocked = AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            summary: "could not apply fix".into(),
            usage: Usage::default(),
            blocked_reason: Some("cannot fix".into()),
        };
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, dirty_review)
            .script(Role::FixApplier, fix_blocked);
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert!(matches!(outcome, IterOutcome::Blocked(_)));
        // The claim is released back to the ready pool for a future attempt.
        assert!(forge
            .labels_of(IssueId(1))
            .contains(&"status:ready".to_string()));
    }

    #[tokio::test]
    async fn unexpected_forge_error_releases_claim() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        // No scripted implementer outcome: run_role returns Err(Backend(..)) after
        // the claim, exercising the release-on-unexpected-error safety net.
        let backend = FakeBackend::new();
        let engine = Engine::new(&cfg, &forge, &backend, PathBuf::from(".")).unwrap();

        let result = engine.run_one().await;
        assert!(result.is_err());
        // The claim was released back to the ready pool despite the hard error.
        assert!(forge
            .labels_of(IssueId(1))
            .contains(&"status:ready".to_string()));
    }

    #[test]
    fn stop_action_is_not_auto_executable() {
        let stop = PlanDecision {
            action: PlanAction::Stop,
            rationale: String::new(),
            needs_human: false,
        };
        assert!(!plan_is_auto_executable(&stop));
    }

    #[test]
    fn planner_guardrail_blocks_close_milestone_and_human_actions() {
        let close = PlanDecision {
            action: PlanAction::CloseMilestone("Phase 2".into()),
            rationale: "".into(),
            needs_human: false,
        };
        assert!(!plan_is_auto_executable(&close));
        let human = PlanDecision {
            action: PlanAction::NextIssue,
            rationale: "".into(),
            needs_human: true,
        };
        assert!(!plan_is_auto_executable(&human));
        let ok = PlanDecision {
            action: PlanAction::NextIssue,
            rationale: "".into(),
            needs_human: false,
        };
        assert!(plan_is_auto_executable(&ok));
    }
}
