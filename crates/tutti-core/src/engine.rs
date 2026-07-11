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
use crate::traits::{AgentBackend, EngineError, Forge, Result, RoutingStrategy};
use crate::workspace::{Workspace, WorkspaceHandle};
use std::path::Path;

/// What one iteration of the loop produced. Drives the outer drain decision.
#[derive(Debug, PartialEq, Eq)]
pub enum IterOutcome {
    Shipped,
    NoReadyWork,
    Blocked(String),
    StoppedCiRed,
    // Reserved for the gate stage, wired with the live implement adapter (not produced in slice 1).
    StoppedGateRed,
}

pub struct Engine<'a> {
    pub cfg: &'a Config,
    pub forge: &'a dyn Forge,
    pub backend: &'a dyn AgentBackend,
    pub routing: Box<dyn RoutingStrategy>,
    /// Creates and tears down an isolated worktree per issue (real git or a fake).
    pub workspace: Box<dyn Workspace>,
}

impl<'a> Engine<'a> {
    pub fn new(
        cfg: &'a Config,
        forge: &'a dyn Forge,
        backend: &'a dyn AgentBackend,
        workspace: Box<dyn Workspace>,
    ) -> Result<Self> {
        let routing = routing::by_name(&cfg.routing, &cfg.integration_branch, &cfg.trunk)
            .ok_or_else(|| EngineError::Routing(format!("unknown routing '{}'", cfg.routing)))?;
        Ok(Self {
            cfg,
            forge,
            backend,
            routing,
            workspace,
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
        worktree: &Path,
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
        let out = self.backend.run(task, worktree, tx).await;
        let _ = drain.await;
        out
    }

    /// Run one issue end-to-end. Returns the iteration outcome. Never merges to trunk.
    pub async fn run_one(&self) -> Result<IterOutcome> {
        // GUARDRAIL #1: selection skips needs-human etc. via SelectFilter.
        let Some(issue) = self.forge.next_ready_issue(&self.cfg.select).await? else {
            return Ok(IterOutcome::NoReadyWork);
        };
        // Bind the guard so the claim's lifetime is clear for the duration of the
        // iteration, even though nothing observes it: release-on-error below is
        // what actually resolves the claim on failure.
        let _guard = self.forge.claim(issue.id).await?;
        let result = self.run_claimed(&issue).await;
        if result.is_err() {
            // Best-effort release so a transient failure does not strand the claim.
            let _ = self.forge.release(issue.id).await;
        }
        result
    }

    /// The claimed body of one iteration. Creates an isolated worktree per issue,
    /// runs the stages there, and removes the worktree on every terminal path.
    /// Any unexpected `Err` propagates and `run_one` releases the claim.
    async fn run_claimed(&self, issue: &Issue) -> Result<IterOutcome> {
        // Routing is pure in the issue, so decide the target branch up front and use it
        // to pick the worktree base.
        let plan = self.routing.target_branch(issue)?;
        let base = if self.forge.branch_exists(&plan.target).await? {
            plan.target.clone()
        } else {
            plan.create_from
                .clone()
                .unwrap_or_else(|| self.cfg.trunk.clone())
        };
        let handle = self.workspace.create(issue.id, &base).await?;

        // Run the stages, then always remove the workspace (Ok or Err).
        let result = self.run_stages(issue, &handle, plan).await;
        let _ = self.workspace.remove(&handle).await;
        result
    }

    /// The stage pipeline for a claimed issue, run inside an already-created worktree.
    async fn run_stages(
        &self,
        issue: &Issue,
        handle: &WorkspaceHandle,
        plan: crate::domain::BranchPlan,
    ) -> Result<IterOutcome> {
        let wt = handle.path.as_path();

        // Stage: implement.
        let impl_out = self.run_role(Role::Implementer, issue, None, wt).await?;
        if impl_out.status != AgentStatus::ReadyToShip {
            self.forge.release(issue.id).await?;
            return Ok(IterOutcome::Blocked(
                impl_out.blocked_reason.unwrap_or_default(),
            ));
        }
        let Some(mut handoff) = impl_out.handoff else {
            self.forge.release(issue.id).await?;
            return Ok(IterOutcome::Blocked(
                "agent reported ReadyToShip but produced no handoff".into(),
            ));
        };

        // Stage: review (fresh agent).
        let review_out = self.run_role(Role::Reviewer, issue, None, wt).await?;
        let report = review_out.review.unwrap_or(ReviewReport {
            findings: vec![],
            verdict: crate::message::Verdict::Approve,
        });

        // Stage: apply-fixes if the review demands it.
        if report.needs_fixes() {
            let fix_out = self
                .run_role(Role::FixApplier, issue, Some(report), wt)
                .await?;
            if fix_out.status != AgentStatus::ReadyToShip {
                self.forge.release(issue.id).await?;
                return Ok(IterOutcome::Blocked(
                    fix_out.blocked_reason.unwrap_or_default(),
                ));
            }
            let Some(fix_handoff) = fix_out.handoff else {
                self.forge.release(issue.id).await?;
                return Ok(IterOutcome::Blocked(
                    "fix applier reported ReadyToShip but produced no handoff".into(),
                ));
            };
            handoff = fix_handoff;
        }

        // The routing strategy decides the target; overwrite whatever the agent guessed.
        handoff.target = plan;

        // Stage: merge (mechanical executor). CI is the gate for real forges;
        // the local `Gate` is run by the implement stage's own tooling before handoff.
        let exec = Executor {
            forge: self.forge,
            trunk: self.cfg.trunk.clone(),
            ci_max_polls: self.cfg.ci_max_polls,
            poll_delay: std::time::Duration::from_secs(self.cfg.poll_delay_secs),
        };
        match exec.ship(&handoff).await? {
            // record() already flipped the label to done.
            ShipResult::Merged(_) => Ok(IterOutcome::Shipped),
            // Leave the issue in-progress and the PR open for a human.
            ShipResult::CiNotGreen(_, _) => Ok(IterOutcome::StoppedCiRed),
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
        // Slice 2: the live planner executes whitelisted actions; slice 1 only proposes.
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
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

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
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
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
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
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
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::NoReadyWork);
    }

    #[tokio::test]
    async fn ci_red_leaves_issue_in_progress() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Fail);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

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
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert!(matches!(outcome, IterOutcome::Blocked(_)));
        // The claim is released back to the ready pool for a future attempt.
        assert!(forge
            .labels_of(IssueId(1))
            .contains(&"status:ready".to_string()));
    }

    #[tokio::test]
    async fn unexpected_backend_error_releases_claim() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        // No scripted implementer outcome: run_role returns Err(Backend(..)) after
        // the claim, exercising the release-on-unexpected-error safety net.
        let backend = FakeBackend::new();
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

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

    struct CountingWorkspace {
        created: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        removed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        last_base: std::sync::Mutex<String>,
    }
    #[async_trait::async_trait]
    impl crate::workspace::Workspace for CountingWorkspace {
        async fn create(
            &self,
            issue: crate::domain::IssueId,
            base: &str,
        ) -> crate::traits::Result<crate::workspace::WorkspaceHandle> {
            self.created
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_base.lock().unwrap() = base.to_string();
            Ok(crate::workspace::WorkspaceHandle {
                issue,
                path: std::path::PathBuf::from("."),
                branch: format!("feat/issue-{}", issue.0),
            })
        }
        async fn remove(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
        ) -> crate::traits::Result<()> {
            self.removed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn prune(&self) -> crate::traits::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn workspace_is_created_and_removed_per_issue() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let created = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let removed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let ws = Box::new(CountingWorkspace {
            created: created.clone(),
            removed: removed.clone(),
            last_base: std::sync::Mutex::new(String::new()),
        });
        let engine = Engine::new(&cfg, &forge, &backend, ws).unwrap();
        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::Shipped);
        assert_eq!(created.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(removed.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
