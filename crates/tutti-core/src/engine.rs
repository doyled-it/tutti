// SPDX-License-Identifier: AGPL-3.0-or-later
//! The drain loop. One issue per iteration: select, implement, then a review verify
//! loop (review, apply-fixes, re-review) that only breaks once the review carries no
//! Blocking/Major finding, or parks the issue for a human once `max_review_iterations`
//! fix cycles pass with a Blocking/Major finding still surviving, gate, merge (via the
//! executor), record, plan.

use crate::config::Config;
use crate::domain::{Issue, IssueState};
use crate::events::{EngineEvent, EngineHooks, SubsessionEvent};
use crate::executor::{Executor, ShipResult};
use crate::message::{
    AgentEvent, AgentOutcome, AgentStatus, AgentTask, PlanAction, PlanDecision, ReviewReport, Role,
    RolePlaybook,
};
use crate::routing;
use crate::traits::{AgentBackend, EngineError, Forge, Result, RoutingStrategy};
use crate::workspace::{Workspace, WorkspaceHandle};
use std::path::Path;

/// The label a blocked issue is parked behind, awaiting a human. This is the engine's
/// park state: `select_ready_issue` always skips it (independent of the operator's
/// `skip_labels`) and `park_for_human` tags a blocked issue with exactly this label.
/// Deliberately fixed rather than drawn from `select.skip_labels`, which is an arbitrary
/// exclusion set that may hold unrelated labels (e.g. `wontfix`).
const NEEDS_HUMAN_LABEL: &str = "status:needs-human";

/// How many issues may block back-to-back before the drain stops. Isolated blocks are
/// parked and the drain continues; but if this many block in a row with no ship between,
/// the failure is treated as systemic (a broken gate or backend) and the drain halts
/// rather than burning an agent run on every remaining ready issue. A ship resets it.
const MAX_CONSECUTIVE_BLOCKS: u32 = 3;

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
    /// Optional context provider (codegraph). `None` = no MCP wiring, prior behavior.
    pub context: Option<&'a dyn crate::context::ContextProvider>,
    /// Optional conventions provider. `None` = no preamble injected, prior behavior.
    pub conventions: Option<&'a dyn crate::conventions::ConventionsProvider>,
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
            context: None,
            conventions: None,
        })
    }

    /// Attach a context provider (e.g. codegraph). Builder-style so existing callers that
    /// do not wire context stay unchanged.
    pub fn with_context(mut self, provider: &'a dyn crate::context::ContextProvider) -> Self {
        self.context = Some(provider);
        self
    }

    /// Attach a conventions provider. Builder-style so existing callers that do not wire
    /// conventions stay unchanged.
    pub fn with_conventions(
        mut self,
        provider: &'a dyn crate::conventions::ConventionsProvider,
    ) -> Self {
        self.conventions = Some(provider);
        self
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
        hooks: &EngineHooks,
    ) -> Result<AgentOutcome> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
        let mcp_servers = if let Some(cx) = self.context {
            cx.ensure_ready().await;
            cx.mcp_servers()
        } else {
            Vec::new()
        };
        let skill_preamble = self
            .conventions
            .and_then(|c| c.preamble_for(role, worktree));
        let task = AgentTask {
            playbook: self.playbook(role),
            issue: issue.clone(),
            worktree_branch: format!("feat/issue-{}", issue.id.0),
            model: self.cfg.model.clone(),
            review,
            mcp_servers,
            skill_preamble,
        };

        let issue_id = issue.id.0;
        hooks.emit_subsession(SubsessionEvent::Started {
            issue: issue_id,
            role,
            title: issue.title.clone(),
        });

        // Forward each AgentEvent out as a SubsessionEvent instead of discarding it. The sink
        // is optional and cloned into the 'static task; issue_id and role are Copy. When the
        // sink is None the task still drains the channel (so the backend never blocks) but
        // sends nothing: behavior identical to the old discard.
        let sink = hooks.subsession.clone();
        let forward = tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let Some(sink) = &sink else { continue };
                let out = match ev {
                    AgentEvent::Line(text) => Some(SubsessionEvent::Delta {
                        issue: issue_id,
                        role,
                        text,
                    }),
                    AgentEvent::ToolUse(name) => Some(SubsessionEvent::Tool {
                        issue: issue_id,
                        role,
                        name,
                    }),
                    AgentEvent::Done => None,
                };
                if let Some(ev) = out {
                    let _ = sink.send(ev);
                }
            }
        });

        let out = self.backend.run(task, worktree, tx).await;
        let _ = forward.await;

        let (summary, ok) = subsession_summary(role, &out);
        hooks.emit_subsession(SubsessionEvent::Completed {
            issue: issue_id,
            role,
            summary,
            ok,
        });
        out
    }

    /// Run one issue end-to-end. Returns the iteration outcome. Never merges to trunk.
    /// Back-compatible entry point: no hooks, so behavior is unchanged for existing
    /// callers (the CLI, existing tests).
    pub async fn run_one(&self) -> Result<IterOutcome> {
        self.run_one_hooked(&EngineHooks::default()).await
    }

    /// `run_one` with lifecycle emission. Identical control flow to `run_one`; only
    /// adds hook calls at claim, ship, and release.
    async fn run_one_hooked(&self, hooks: &EngineHooks) -> Result<IterOutcome> {
        // GUARDRAIL #1: selection skips needs-human etc. via SelectFilter.
        let Some(issue) = self.select_ready_issue().await? else {
            return Ok(IterOutcome::NoReadyWork);
        };
        // Bind the guard so the claim's lifetime is clear for the duration of the
        // iteration, even though nothing observes it: release-on-error below is
        // what actually resolves the claim on failure.
        let _guard = self.forge.claim(issue.id).await?;
        hooks.emit(EngineEvent::IssueClaimed {
            id: issue.id.0,
            title: issue.title.clone(),
        });
        let result = self.run_claimed(&issue, hooks).await;
        if result.is_err() {
            // Best-effort release so a transient failure does not strand the claim.
            hooks.emit(EngineEvent::IssueReleased { id: issue.id.0 });
            let _ = self.forge.release(issue.id).await;
        }
        result
    }

    /// Pick the issue this iteration works on, applying the milestone floor if it is on.
    ///
    /// The floor asks the same selector once per open milestone, earliest first, and takes
    /// the first hit. "This milestone is drained" therefore means "it has no ready issue
    /// left", which is a broader and more useful test here than `Progress::is_drained`: an
    /// issue parked behind `status:needs-human` should not pin the floor to a milestone the
    /// loop cannot make progress on.
    ///
    /// The last probe is deliberately unscoped. That is what makes the floor *soft*: once
    /// no open milestone has ready work, issues that carry no milestone at all still get
    /// picked up, so turning the floor on can reorder the drain but can never starve it.
    ///
    /// Cost: one `list_ready_issues`, plus one `list_milestones` when the floor is on. The
    /// ranking itself is pure.
    ///
    /// The first version of this asked the selector once per open milestone. Every adapter
    /// implements selection as one list fetch filtered in process, so that was M identical
    /// fetches per iteration with M-1 discarded — on GitHub, M `gh` subprocess spawns and M
    /// API round trips to answer a question one fetch already contains. The floor is pure
    /// ordering over a set the forge hands over in a single call, so it is applied here as
    /// exactly that.
    async fn select_ready_issue(&self) -> Result<Option<Issue>> {
        let ready = self.forge.list_ready_issues(&self.select_filter()).await?;
        if !self.cfg.select.milestone_floor || self.cfg.select.milestone.is_some() {
            return Ok(ready.into_iter().next());
        }
        let milestones = self.forge.list_milestones().await?;
        let order = crate::tracking::milestone_floor_order(&milestones);
        Ok(pick_by_milestone_floor(ready, &order))
    }

    /// The claimed body of one iteration. Creates an isolated worktree per issue,
    /// runs the stages there, and removes the worktree on every terminal path.
    /// Any unexpected `Err` propagates and `run_one_hooked` releases the claim.
    async fn run_claimed(&self, issue: &Issue, hooks: &EngineHooks) -> Result<IterOutcome> {
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
        let result = self.run_stages(issue, &handle, plan, &base, hooks).await;
        let _ = self.workspace.remove(&handle).await;
        result
    }

    /// Park a blocked issue for a human rather than returning it to the ready pool.
    /// Adds the configured skip label(s) (e.g. `status:needs-human`) and removes the
    /// in-progress label, so the drain's selector no longer sees it and moves on to
    /// the next issue instead of re-selecting the same block. With no skip label
    /// configured it still drops the in-progress label, leaving the issue out of the
    /// ready set. Emits the release event for lifecycle observers.
    /// The configured select filter with the needs-human park label guaranteed present in
    /// the skip set, so a parked issue is never re-selected even when the operator did not
    /// list the park label in `skip_labels` (or left it empty).
    fn select_filter(&self) -> crate::domain::SelectFilter {
        let mut filter = self.cfg.select.clone();
        if !filter.skip_labels.iter().any(|s| s == NEEDS_HUMAN_LABEL) {
            filter.skip_labels.push(NEEDS_HUMAN_LABEL.to_string());
        }
        filter
    }

    async fn park_for_human(&self, id: crate::domain::IssueId, hooks: &EngineHooks) -> Result<()> {
        hooks.emit(EngineEvent::IssueParked { id: id.0 });
        // Add exactly the needs-human park label (not the whole skip set) and drop the
        // in-progress label, so the selector no longer sees the issue and a human can
        // find it by the park label.
        let park = NEEDS_HUMAN_LABEL.to_string();
        let in_progress = self.cfg.status_labels().in_progress;
        self.forge
            .edit_labels(
                id,
                std::slice::from_ref(&park),
                std::slice::from_ref(&in_progress),
            )
            .await
    }

    /// The stage pipeline for a claimed issue, run inside an already-created worktree.
    async fn run_stages(
        &self,
        issue: &Issue,
        handle: &WorkspaceHandle,
        plan: crate::domain::BranchPlan,
        base: &str,
        hooks: &EngineHooks,
    ) -> Result<IterOutcome> {
        let wt = handle.path.as_path();

        // Stage: implement.
        let impl_out = self
            .run_role(Role::Implementer, issue, None, wt, hooks)
            .await?;
        if impl_out.status != AgentStatus::ReadyToShip {
            self.park_for_human(issue.id, hooks).await?;
            return Ok(IterOutcome::Blocked(
                impl_out.blocked_reason.unwrap_or_default(),
            ));
        }
        let Some(mut handoff) = impl_out.handoff else {
            self.park_for_human(issue.id, hooks).await?;
            return Ok(IterOutcome::Blocked(
                "agent reported ReadyToShip but produced no handoff".into(),
            ));
        };

        // Stage: review + verify loop. Re-review every fix; a clean review (no
        // Blocking/Major findings) is a hard precondition for the ship. Minor findings
        // never gate the ship, extend the loop, or trigger a fix pass: they are advisory
        // notes the opinionated gate and conventions are meant to design away, not mutate
        // an already-reviewed tree for. Park after `max_review_iterations` fix cycles if a
        // Blocking/Major finding survives.
        let mut iterations: u32 = 0;
        loop {
            let review_out = self
                .run_role(Role::Reviewer, issue, None, wt, hooks)
                .await?;
            let report = review_out.review.unwrap_or(ReviewReport {
                findings: vec![],
                verdict: crate::message::Verdict::Approve,
            });

            if !report.has_blocking_or_major() {
                // Ship the already-reviewed tree as-is; no fix pass, no further review.
                break;
            }

            iterations += 1;
            if iterations > self.cfg.max_review_iterations {
                self.park_for_human(issue.id, hooks).await?;
                return Ok(IterOutcome::Blocked(
                    "review did not converge: a blocking or major finding survived the fix \
                     budget"
                        .into(),
                ));
            }
            let fix_out = self
                .run_role(Role::FixApplier, issue, Some(report), wt, hooks)
                .await?;
            if fix_out.status != AgentStatus::ReadyToShip {
                self.park_for_human(issue.id, hooks).await?;
                return Ok(IterOutcome::Blocked(
                    fix_out.blocked_reason.unwrap_or_default(),
                ));
            }
            let Some(fix_handoff) = fix_out.handoff else {
                self.park_for_human(issue.id, hooks).await?;
                return Ok(IterOutcome::Blocked(
                    "fix applier reported ReadyToShip but produced no handoff".into(),
                ));
            };
            handoff = fix_handoff;
            // loop back and re-review the fix
        }

        // The routing strategy decides the target; overwrite whatever the agent guessed.
        handoff.target = plan;

        // Commit the agent's working-tree changes onto the feature branch. The review
        // stage above reads the uncommitted tree, which is fine. The agent may also
        // commit its own work (the superpowers workflow does), leaving a clean tree:
        // ship when EITHER commit_all committed uncommitted changes OR the branch
        // already carries the agent's commits beyond its base. Block only when there
        // is genuinely nothing to ship.
        let committed = self.workspace.commit_all(handle, &handoff.pr_title).await?;
        if !committed && !self.workspace.has_commits(handle, base).await? {
            self.park_for_human(issue.id, hooks).await?;
            return Ok(IterOutcome::Blocked(
                "agent produced no file changes to commit".into(),
            ));
        }

        // Stage: merge (mechanical executor). CI is the gate for real forges;
        // the local `Gate` is run by the implement stage's own tooling before handoff.
        let exec = Executor {
            forge: self.forge,
            trunk: self.cfg.trunk.clone(),
            ci_max_polls: self.cfg.ci_max_polls,
            poll_delay: std::time::Duration::from_secs(self.cfg.poll_delay_secs),
            merge_mode: self.cfg.merge_mode,
        };
        match exec.ship(&handoff).await? {
            // record() already flipped the label to done.
            ShipResult::Merged(_) => {
                self.maybe_close_milestone(issue).await?;
                hooks.emit(EngineEvent::IssueShipped { id: issue.id.0 });
                Ok(IterOutcome::Shipped)
            }
            // Leave the issue in-progress and the PR open for a human.
            ShipResult::CiNotGreen(_, _) => Ok(IterOutcome::StoppedCiRed),
        }
    }

    /// After a ship, close the shipped issue's milestone iff it is verifiably drained:
    /// resolve the milestone by title, read its children fresh at close time, and close
    /// only when that child set is NON-EMPTY and every child carries `status:done`.
    /// A milestone with no milestone on the issue, an unresolvable title, an empty child
    /// set, or any remaining open work is left untouched. The check is read live (never a
    /// cached count) so a stale rollup can never trigger a close.
    async fn maybe_close_milestone(&self, issue: &Issue) -> Result<()> {
        let Some(title) = issue.milestone.as_deref() else {
            return Ok(());
        };
        let Some(milestone) = self
            .forge
            .list_milestones()
            .await?
            .into_iter()
            .find(|m| m.title == title)
        else {
            return Ok(());
        };
        let children = self.forge.milestone_children(milestone.id).await?;
        let drained = !children.is_empty() && children.iter().all(|c| c.has_label("status:done"));
        if drained {
            self.forge.close_milestone(milestone.id).await?;
        }
        Ok(())
    }

    /// Drain up to `max_issues_per_run` issues, then run the planning hook once and
    /// execute whatever it decided (subject to the whitelist). Returns the number of
    /// issues shipped and the planner's decision (if the planner ran at all).
    /// Back-compatible entry point (no hooks).
    pub async fn drain(&self) -> Result<(u32, Option<PlanDecision>)> {
        self.drain_with(&EngineHooks::default()).await
    }

    /// Drain with live event + cancel hooks. Emits `DrainStarted`/`DrainComplete` around
    /// the loop and checks cancellation between issues (finish-then-stop).
    pub async fn drain_with(&self, hooks: &EngineHooks) -> Result<(u32, Option<PlanDecision>)> {
        hooks.emit(EngineEvent::DrainStarted);
        let mut shipped = 0;
        let mut consecutive_blocks = 0u32;
        for _ in 0..self.cfg.max_issues_per_run {
            if hooks.cancelled() {
                break;
            }
            match self.run_one_hooked(hooks).await? {
                IterOutcome::Shipped => {
                    shipped += 1;
                    consecutive_blocks = 0;
                }
                IterOutcome::NoReadyWork => break,
                // A blocked issue is parked for a human (dropped from the ready pool);
                // keep draining so one bad issue cannot starve the rest. But if enough
                // block back-to-back with no ship between, treat the failure as systemic
                // and stop rather than burning an agent run on every ready issue.
                IterOutcome::Blocked(_) => {
                    consecutive_blocks += 1;
                    if consecutive_blocks >= MAX_CONSECUTIVE_BLOCKS {
                        break;
                    }
                    continue;
                }
                // A red CI or gate leaves a PR open: a genuine stop-for-human.
                _ => break,
            }
        }
        let plan = if shipped > 0 {
            let decision = self.plan(hooks).await?;
            self.execute_plan(&decision).await?;
            Some(decision)
        } else {
            None
        };
        hooks.emit(EngineEvent::DrainComplete { shipped });
        Ok((shipped, plan))
    }

    /// Build a compact tracking snapshot for the planner: milestones with their progress
    /// rollups, plus a ready-issue count. Titles and progress only, never issue bodies, so
    /// the prompt stays small.
    async fn plan_snapshot(&self) -> Result<String> {
        let milestones = self.forge.list_milestones().await?;
        // epics omitted from the snapshot to avoid an N+1 in the planning hot path;
        // revisit when list_epics is cheap.

        // Count ready issues across the tracked milestones (best effort: the Forge trait
        // exposes issues only via milestone children and the single-issue selector). This
        // is a size hint for the planner, not an exact global inventory.
        let mut ready_issues = 0u32;
        for m in &milestones {
            for child in self.forge.milestone_children(m.id).await? {
                if child.has_label(&self.cfg.select.require_label) {
                    ready_issues += 1;
                }
            }
        }

        let mut s = String::from("Project tracking snapshot.\n\nMilestones:\n");
        if milestones.is_empty() {
            s.push_str("  (none)\n");
        }
        for m in &milestones {
            s.push_str(&format!(
                "  - {} [{:?}] progress {}/{}\n",
                m.title, m.state, m.progress.done, m.progress.total
            ));
        }
        s.push_str(&format!("Ready issues: {ready_issues}\n"));
        Ok(s)
    }

    /// The planning hook. Runs the Planner role against the live tracking snapshot and
    /// returns its `PlanDecision`. The planner needs no isolated worktree, so it runs in
    /// the repo/working dir. If the planner produced no decision, default to `Stop` so the
    /// loop halts safely rather than acting on nothing.
    async fn plan(&self, hooks: &EngineHooks) -> Result<PlanDecision> {
        let snapshot = self.plan_snapshot().await?;
        // A synthetic issue carries the snapshot as context; id 0 is the planner sentinel.
        let planner_issue = Issue {
            id: crate::domain::IssueId(0),
            title: "Plan the next action for this project".into(),
            body: snapshot,
            labels: vec![],
            milestone: None,
            state: IssueState::Open,
        };
        let workdir = self.cfg.gate.working_dir.as_path();
        let workdir = if workdir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            workdir
        };
        let outcome = self
            .run_role(Role::Planner, &planner_issue, None, workdir, hooks)
            .await?;
        Ok(outcome.plan.unwrap_or(PlanDecision {
            action: PlanAction::Stop,
            rationale: "planner produced no decision".into(),
            needs_human: false,
        }))
    }

    /// Execute a plan decision through GUARDRAIL #3. Only whitelisted, non-human actions
    /// run: `NextIssue` (the drain loop already continues) and `CreateIssues` (created via
    /// the forge). `CloseMilestone` and any `needs_human` decision are surfaced (returned to
    /// the caller in the drain result), never auto-executed here: milestone auto-close is
    /// owned exclusively by the verified-drain path (`maybe_close_milestone`).
    async fn execute_plan(&self, decision: &PlanDecision) -> Result<()> {
        if !plan_is_auto_executable(decision) {
            // Surfaced, not executed. The decision is returned to the caller for a human.
            return Ok(());
        }
        match &decision.action {
            PlanAction::NextIssue => {}
            PlanAction::CreateIssues(list) => {
                // Resolve the planner's title hints to forge ids once for the whole batch,
                // not once per issue, and only fetch a list some issue actually references.
                // `list_epics` in particular is an N+1 on GitHub, so an epic-free plan (the
                // common case today) must not pay for it.
                let milestones = if list.iter().any(|n| n.milestone.is_some()) {
                    self.forge.list_milestones().await?
                } else {
                    Vec::new()
                };
                let epics = if list.iter().any(|n| n.epic.is_some()) {
                    self.forge.list_epics().await?
                } else {
                    Vec::new()
                };
                for new in list {
                    let milestone = new
                        .milestone
                        .as_deref()
                        .and_then(|t| resolve_by_title(&milestones, t, |m| &m.title))
                        .map(|m| m.id);
                    let epic = new
                        .epic
                        .as_deref()
                        .and_then(|t| resolve_by_title(&epics, t, |e| &e.title))
                        .map(|e| e.id);
                    self.forge.create_issue(new, milestone, epic).await?;
                }
            }
            // Non-whitelisted actions never reach here (guarded by plan_is_auto_executable).
            PlanAction::CloseMilestone(_) | PlanAction::Stop => {}
        }
        Ok(())
    }
}

/// Apply the milestone floor to an already-fetched set of selectable issues.
///
/// Walks `order` (open milestones, earliest first) and takes the first issue belonging to
/// each in turn. Falling off the end returns the first issue overall, which is what makes
/// the floor *soft*: once no open milestone has ready work, issues that carry no milestone
/// — or one that has since been closed — are still picked up, so turning the floor on can
/// reorder a drain but never starve it.
///
/// Pure, and takes the whole candidate set rather than a forge, because the floor is
/// ordering rather than a query. That is the entire reason this is one fetch and not M.
fn pick_by_milestone_floor(
    ready: Vec<Issue>,
    order: &[&crate::tracking::Milestone],
) -> Option<Issue> {
    for m in order {
        if let Some(i) = ready
            .iter()
            .position(|i| i.milestone.as_deref() == Some(&m.title))
        {
            return ready.into_iter().nth(i);
        }
    }
    ready.into_iter().next()
}

/// Resolve a planner-supplied title to the tracking item it names.
///
/// Exact match first, then a case-insensitive pass. The planner retypes titles it read out
/// of the tracking snapshot, and an LLM retyping `v0.1 MVP` as `v0.1 mvp` should still land
/// the issue in the right milestone. Returning `None` is not an error: the caller files the
/// issue at the top level, where a human will see it, rather than dropping proposed work.
fn resolve_by_title<'a, T>(
    items: &'a [T],
    title: &str,
    name: impl Fn(&T) -> &str,
) -> Option<&'a T> {
    items
        .iter()
        .find(|it| name(it) == title)
        .or_else(|| items.iter().find(|it| name(it).eq_ignore_ascii_case(title)))
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

/// Compose the one-line outcome shown under a subsession, plus whether it succeeded (drives
/// the status dot). Role-aware because each role's "result" lives in a different field of the
/// outcome. Pure, so it is unit-tested directly.
pub(crate) fn subsession_summary(role: Role, out: &Result<AgentOutcome>) -> (String, bool) {
    let outcome = match out {
        Ok(o) => o,
        Err(e) => return (format!("error: {e}"), false),
    };
    match role {
        Role::Implementer | Role::FixApplier | Role::Greener => {
            if outcome.status == AgentStatus::ReadyToShip {
                ("ready to ship".to_string(), true)
            } else {
                let reason = outcome
                    .blocked_reason
                    .as_deref()
                    .unwrap_or("no reason given");
                (format!("blocked: {reason}"), false)
            }
        }
        Role::Reviewer => match &outcome.review {
            Some(r) if r.has_blocking_or_major() => {
                let n = r
                    .findings
                    .iter()
                    .filter(|f| {
                        matches!(
                            f.severity,
                            crate::message::Severity::Blocking | crate::message::Severity::Major
                        )
                    })
                    .count();
                let noun = if n == 1 { "finding" } else { "findings" };
                (format!("changes needed ({n} {noun})"), false)
            }
            Some(_) => ("approved".to_string(), true),
            // Mirror what `run_stages` actually does with a missing report: it substitutes an
            // empty Approve and ships. Reporting this as an error put a red dot on a stage the
            // engine had approved, so the pane contradicted the run it is a window onto.
            // Whether shipping on a missing report is the RIGHT engine behaviour is a separate
            // question; the pane's job is to say what happened, not to editorialise.
            None => ("approved (no report written)".to_string(), true),
        },
        Role::Planner => match &outcome.plan {
            Some(d) => (format!("plan: {}", plan_action_label(&d.action)), true),
            None => ("no decision".to_string(), false),
        },
    }
}

/// A short, stable label for a plan action (avoids leaking `{:?}` of the whole `CreateIssues`
/// vector into the UI).
fn plan_action_label(action: &PlanAction) -> &'static str {
    match action {
        PlanAction::NextIssue => "next issue",
        PlanAction::CreateIssues(_) => "create issues",
        PlanAction::CloseMilestone(_) => "close milestone",
        PlanAction::Stop => "stop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::domain::{BranchPlan, CiState, Issue, IssueId, SelectFilter};
    use crate::gate::Gate;
    use crate::message::*;
    use crate::testing::{FakeBackend, FakeForge};
    use crate::tracking::TrackState;

    fn cfg() -> Config {
        Config {
            trunk: "main".into(),
            routing: "trunk".into(),
            integration_branch: "version/v0.1".into(),
            model: "fake".into(),
            max_issues_per_run: 5,
            max_review_iterations: 3,
            ci_max_polls: 40,
            poll_delay_secs: 0,
            select: SelectFilter {
                require_label: "status:ready".into(),
                skip_labels: vec!["status:needs-human".into()],
                milestone: None,
                milestone_floor: false,
            },
            gate: Gate {
                commands: vec!["true".into()],
                working_dir: Default::default(),
            },
            status: None,
            forge: Default::default(),
            roles: crate::config::default_roles(),
            merge_mode: crate::domain::MergeMode::Merge,
            codegraph: None,
        }
    }

    fn ready(id: u64) -> Issue {
        Issue {
            id: IssueId(id),
            title: format!("i{id}"),
            body: String::new(),
            labels: vec!["status:ready".into()],
            milestone: None,
            state: IssueState::Open,
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
            plan: None,
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
            plan: None,
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
            plan: None,
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        // With the verify loop, a Blocking finding forces a fix and a re-review; only
        // two scripted Reviewer outcomes are given, so a Shipped result here also proves
        // the loop stopped after the second (clean) review rather than asking for a third.
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, dirty_review)
            .script(Role::FixApplier, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
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
    async fn review_reraises_until_clean_then_ships() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let dirty_review = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Major,
                    file: "a.rs".into(),
                    line: None,
                    claim: "wrong condition".into(),
                }],
                verdict: Verdict::RequestChanges,
            }),
            plan: None,
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        // Only two Reviewer outcomes are scripted: the dirty one and a trailing clean
        // one. A Shipped result proves the fix was re-reviewed exactly once more and the
        // loop stopped there, since a third Reviewer call with nothing scripted errors.
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, dirty_review)
            .script(Role::FixApplier, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
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
    async fn review_parks_after_max_iterations() {
        let mut cfg = cfg();
        cfg.max_review_iterations = 2;
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let dirty_review = || AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Major,
                    file: "a.rs".into(),
                    line: None,
                    claim: "wrong condition".into(),
                }],
                verdict: Verdict::RequestChanges,
            }),
            plan: None,
            summary: "still wrong".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        // A finding survives every fix cycle: 3 Major reviews against a cap of 2, with a
        // ship fix after each of the first two, is enough to exhaust the budget and park.
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, dirty_review())
            .script(Role::FixApplier, ship_outcome(1))
            .script(Role::Reviewer, dirty_review())
            .script(Role::FixApplier, ship_outcome(1))
            .script(Role::Reviewer, dirty_review());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert!(
            matches!(outcome, IterOutcome::Blocked(_)),
            "got {outcome:?}"
        );
        assert!(!forge.is_done(IssueId(1)));
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
    }

    #[tokio::test]
    async fn minor_only_review_ships_without_a_fix_pass() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let minor_review = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Minor,
                    file: "a.rs".into(),
                    line: None,
                    claim: "small coverage gap".into(),
                }],
                verdict: Verdict::Approve,
            }),
            plan: None,
            summary: "minor note".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        // No FixApplier outcome is scripted at all, so a Shipped result proves a
        // Minor-only report ships the already-reviewed tree as-is with no fix pass and
        // no re-review: an unscripted FixApplier or Reviewer call would error instead.
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, minor_review);
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
    async fn blocked_implementer_parks_the_issue_for_a_human() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let blocked = AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            plan: None,
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
        // Parked for a human (needs-human), not returned to the ready pool.
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
        assert!(!labels.contains(&"status:ready".to_string()));
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
    async fn fix_applier_blocked_parks_the_issue() {
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
            plan: None,
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        let fix_blocked = AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            plan: None,
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
        // A fix-applier block parks the issue for a human, not back to ready: with the
        // drain continuing past a block, releasing to ready would re-select and re-block.
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
        assert!(!labels.contains(&"status:ready".to_string()));
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
        async fn commit_all(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
            _message: &str,
        ) -> crate::traits::Result<bool> {
            Ok(true)
        }
        async fn has_commits(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
            _base: &str,
        ) -> crate::traits::Result<bool> {
            Ok(false)
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

    /// Forwards to a shared `CountingWorkspace` so the test can read `last_base`
    /// back after the `Box<dyn Workspace>` has been moved into the engine.
    struct SharedCountingWorkspace(std::sync::Arc<CountingWorkspace>);
    #[async_trait::async_trait]
    impl crate::workspace::Workspace for SharedCountingWorkspace {
        async fn create(
            &self,
            issue: crate::domain::IssueId,
            base: &str,
        ) -> crate::traits::Result<crate::workspace::WorkspaceHandle> {
            self.0.create(issue, base).await
        }
        async fn commit_all(
            &self,
            h: &crate::workspace::WorkspaceHandle,
            message: &str,
        ) -> crate::traits::Result<bool> {
            self.0.commit_all(h, message).await
        }
        async fn has_commits(
            &self,
            h: &crate::workspace::WorkspaceHandle,
            base: &str,
        ) -> crate::traits::Result<bool> {
            self.0.has_commits(h, base).await
        }
        async fn remove(&self, h: &crate::workspace::WorkspaceHandle) -> crate::traits::Result<()> {
            self.0.remove(h).await
        }
        async fn prune(&self) -> crate::traits::Result<()> {
            self.0.prune().await
        }
    }

    #[tokio::test]
    async fn worktree_base_prefers_existing_integration_branch_else_trunk() {
        // Default case: the integration branch does not exist yet, so the worktree
        // base falls back to create_from (trunk, "main").
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let counting = std::sync::Arc::new(CountingWorkspace {
            created: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            removed: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            last_base: std::sync::Mutex::new(String::new()),
        });
        let ws = Box::new(SharedCountingWorkspace(counting.clone()));
        let engine = Engine::new(&cfg, &forge, &backend, ws).unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::Shipped);
        assert_eq!(*counting.last_base.lock().unwrap(), "main");

        // Second scenario: pre-create the integration branch so branch_exists is
        // true, and confirm the worktree base becomes the target branch itself.
        let forge2 = FakeForge::new(vec![ready(1)], CiState::Pass);
        forge2.create_branch("version/v0.1", "main").await.unwrap();
        assert!(forge2.branch_exists("version/v0.1").await.unwrap());
        let backend2 = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let counting2 = std::sync::Arc::new(CountingWorkspace {
            created: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            removed: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            last_base: std::sync::Mutex::new(String::new()),
        });
        let ws2 = Box::new(SharedCountingWorkspace(counting2.clone()));
        let engine2 = Engine::new(&cfg, &forge2, &backend2, ws2).unwrap();

        let outcome2 = engine2.run_one().await.unwrap();
        assert_eq!(outcome2, IterOutcome::Shipped);
        assert_eq!(*counting2.last_base.lock().unwrap(), "version/v0.1");
    }

    /// A workspace whose `commit_all` always reports nothing to commit, so the
    /// engine must treat the issue as producing no shippable work.
    struct NoChangesWorkspace;
    #[async_trait::async_trait]
    impl crate::workspace::Workspace for NoChangesWorkspace {
        async fn create(
            &self,
            issue: crate::domain::IssueId,
            _base: &str,
        ) -> crate::traits::Result<crate::workspace::WorkspaceHandle> {
            Ok(crate::workspace::WorkspaceHandle {
                issue,
                path: std::path::PathBuf::from("."),
                branch: format!("feat/issue-{}", issue.0),
            })
        }
        async fn commit_all(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
            _message: &str,
        ) -> crate::traits::Result<bool> {
            Ok(false)
        }
        async fn has_commits(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
            _base: &str,
        ) -> crate::traits::Result<bool> {
            Ok(false)
        }
        async fn remove(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
        ) -> crate::traits::Result<()> {
            Ok(())
        }
        async fn prune(&self) -> crate::traits::Result<()> {
            Ok(())
        }
    }

    /// A workspace that mimics the superpowers workflow: the agent committed its
    /// own work, so the worktree is clean (`commit_all` finds nothing) but the
    /// branch carries commits beyond its base.
    struct PreCommittedWorkspace;
    #[async_trait::async_trait]
    impl crate::workspace::Workspace for PreCommittedWorkspace {
        async fn create(
            &self,
            issue: crate::domain::IssueId,
            _base: &str,
        ) -> crate::traits::Result<crate::workspace::WorkspaceHandle> {
            Ok(crate::workspace::WorkspaceHandle {
                issue,
                path: std::path::PathBuf::from("."),
                branch: format!("feat/issue-{}", issue.0),
            })
        }
        async fn commit_all(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
            _message: &str,
        ) -> crate::traits::Result<bool> {
            Ok(false)
        }
        async fn has_commits(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
            _base: &str,
        ) -> crate::traits::Result<bool> {
            Ok(true)
        }
        async fn remove(
            &self,
            _h: &crate::workspace::WorkspaceHandle,
        ) -> crate::traits::Result<()> {
            Ok(())
        }
        async fn prune(&self) -> crate::traits::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn no_shippable_work_parks_the_issue_for_a_human() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(&cfg, &forge, &backend, Box::new(NoChangesWorkspace)).unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert!(matches!(outcome, IterOutcome::Blocked(_)));
        // No commit and no commits on the branch means no ship: not done.
        assert!(!forge.is_done(IssueId(1)));
        // Parked for a human (needs-human), not returned to the ready pool.
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
        assert!(!labels.contains(&"status:ready".to_string()));
    }

    #[tokio::test]
    async fn ships_when_agent_committed_its_own_work() {
        // The superpowers workflow has the agent commit its own work, leaving a
        // clean worktree. `commit_all` then finds nothing to commit, but the branch
        // carries the agent's commits, so the engine must ship rather than block.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(&cfg, &forge, &backend, Box::new(PreCommittedWorkspace)).unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::Shipped);
        assert!(forge.is_done(IssueId(1)));
    }

    #[tokio::test]
    async fn drain_parks_a_blocked_issue_and_continues_to_the_next() {
        // Issue 1's implementer blocks; issue 2 ships. One block must not halt the
        // whole drain: the blocked issue is parked for a human (dropped from the ready
        // pool) and the drain moves on to ship issue 2.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1), ready(2)], CiState::Pass);
        let blocked = AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            plan: None,
            summary: "needs hardware".into(),
            usage: Usage::default(),
            blocked_reason: Some("needs a device".into()),
        };
        let backend = FakeBackend::new()
            .script(Role::Implementer, blocked)
            .script(Role::Implementer, ship_outcome(2))
            .script(Role::Reviewer, clean_review())
            .script(
                Role::Planner,
                planned(PlanDecision {
                    action: PlanAction::Stop,
                    rationale: "nothing left".into(),
                    needs_human: false,
                }),
            );
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let (shipped, _plan) = engine.drain().await.unwrap();
        assert_eq!(shipped, 1);
        assert!(forge.is_done(IssueId(2)));
        assert!(!forge.is_done(IssueId(1)));
        // Issue 1 is parked for a human, not returned to the ready pool.
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
        assert!(!labels.contains(&"status:ready".to_string()));
    }

    fn blocked_outcome() -> AgentOutcome {
        AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            plan: None,
            summary: "blocked".into(),
            usage: Usage::default(),
            blocked_reason: Some("needs a human".into()),
        }
    }

    #[tokio::test]
    async fn park_adds_only_the_needs_human_label_not_the_whole_skip_set() {
        // skip_labels may carry unrelated exclusions (e.g. wontfix). Parking a blocked
        // issue must tag it only with the needs-human park label, never the rest.
        let mut cfg = cfg();
        cfg.select.skip_labels = vec!["status:needs-human".into(), "wontfix".into()];
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new().script(Role::Implementer, blocked_outcome());
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
        let labels = forge.labels_of(IssueId(1));
        assert!(labels.contains(&"status:needs-human".to_string()));
        assert!(!labels.contains(&"wontfix".to_string()));
    }

    #[tokio::test]
    async fn selection_always_skips_the_needs_human_label_even_when_not_configured() {
        // An issue carrying both ready and needs-human (a human parked it without removing
        // ready) must not be selected, even when the operator left the park label out of
        // skip_labels entirely.
        let mut cfg = cfg();
        cfg.select.skip_labels = vec![];
        let mut issue = ready(1);
        issue.labels.push("status:needs-human".into());
        let forge = FakeForge::new(vec![issue], CiState::Pass);
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
    async fn drain_stops_after_max_consecutive_blocks() {
        // Four ready issues, every implementer blocks. The drain parks the first
        // MAX_CONSECUTIVE_BLOCKS and then stops, leaving the rest untouched, rather than
        // burning an agent run on every ready issue for a systemic failure. Only three
        // outcomes are scripted, so a fourth attempt would error the backend and fail.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1), ready(2), ready(3), ready(4)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, blocked_outcome())
            .script(Role::Implementer, blocked_outcome())
            .script(Role::Implementer, blocked_outcome());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
        let (shipped, plan) = engine.drain().await.unwrap();
        assert_eq!(shipped, 0);
        assert!(plan.is_none());
        // The fourth issue was never attempted: still ready, never parked.
        let fourth = forge.labels_of(IssueId(4));
        assert!(fourth.contains(&"status:ready".to_string()));
        assert!(!fourth.contains(&"status:needs-human".to_string()));
        // The first was parked.
        assert!(forge
            .labels_of(IssueId(1))
            .contains(&"status:needs-human".to_string()));
    }

    #[tokio::test]
    async fn blocking_emits_issue_parked_not_issue_released() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new().script(Role::Implementer, blocked_outcome());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = EngineHooks {
            sink: Some(tx),
            cancel: None,
            subsession: None,
        };
        let _ = engine.drain_with(&hooks).await.unwrap();
        let mut evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            evs.push(ev);
        }
        assert!(evs.contains(&EngineEvent::IssueParked { id: 1 }));
        assert!(!evs
            .iter()
            .any(|e| matches!(e, EngineEvent::IssueReleased { .. })));
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
    async fn milestone_auto_closes_when_all_issues_done() {
        // Seed a milestone with two ready issues under it, then ship both. Once the
        // second ship drains the milestone (both children status:done), it must close.
        let cfg = cfg();
        let forge = FakeForge::new(vec![], CiState::Pass);
        let ms = forge.create_milestone("v0.1", None, "").await.unwrap();
        let a = forge
            .create_issue(&new_issue("a"), Some(ms.id), None)
            .await
            .unwrap();
        let b = forge
            .create_issue(&new_issue("b"), Some(ms.id), None)
            .await
            .unwrap();
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(a.id.0))
            .script(Role::Reviewer, clean_review())
            .script(Role::Implementer, ship_outcome(b.id.0))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::Shipped);
        // First ship leaves one child still open: milestone stays open.
        assert_eq!(forge.milestone_state(ms.id), TrackState::Open);
        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::Shipped);
        // Second ship drains the milestone: verified all-done, non-empty, closed now.
        assert_eq!(forge.milestone_state(ms.id), TrackState::Closed);
    }

    #[tokio::test]
    async fn milestone_stays_open_with_remaining_work() {
        // Two issues in the milestone, ship only one: the drain is not verified, so the
        // milestone must remain open.
        let cfg = cfg();
        let forge = FakeForge::new(vec![], CiState::Pass);
        let ms = forge.create_milestone("v0.1", None, "").await.unwrap();
        let a = forge
            .create_issue(&new_issue("a"), Some(ms.id), None)
            .await
            .unwrap();
        let _b = forge
            .create_issue(&new_issue("b"), Some(ms.id), None)
            .await
            .unwrap();
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(a.id.0))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::Shipped);
        assert_eq!(forge.milestone_state(ms.id), TrackState::Open);
    }

    #[tokio::test]
    async fn next_ready_issue_honors_milestone_scope() {
        // One ready issue in "v0.1", one ready issue with no milestone. A scoped filter
        // returns only the in-milestone issue; an unscoped filter returns the first ready.
        let scoped_issue = Issue {
            id: IssueId(1),
            title: "scoped".into(),
            body: String::new(),
            labels: vec!["status:ready".into()],
            milestone: Some("v0.1".into()),
            state: IssueState::Open,
        };
        let unscoped_issue = Issue {
            id: IssueId(2),
            title: "unscoped".into(),
            body: String::new(),
            labels: vec!["status:ready".into()],
            milestone: None,
            state: IssueState::Open,
        };
        let forge = FakeForge::new(
            vec![unscoped_issue.clone(), scoped_issue.clone()],
            CiState::Pass,
        );

        let scoped = SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec![],
            milestone: Some("v0.1".into()),
            milestone_floor: false,
        };
        let got = forge.next_ready_issue(&scoped).await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(1));

        let unscoped = SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec![],
            milestone: None,
            milestone_floor: false,
        };
        // With no scope, selection order is unchanged: the first seeded ready issue.
        let got = forge.next_ready_issue(&unscoped).await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(2));
    }

    /// A scripted Planner outcome carrying `decision`.
    fn planned(decision: PlanDecision) -> AgentOutcome {
        AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: None,
            plan: Some(decision),
            summary: "planned".into(),
            usage: Usage::default(),
            blocked_reason: None,
        }
    }

    #[tokio::test]
    async fn planner_create_issues_are_created_via_forge() {
        // Ship one standalone issue so the planner runs, then have the planner ask to
        // create a follow-up issue. The engine must create it through the forge.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        assert_eq!(forge.issue_count(), 1);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review())
            .script(
                Role::Planner,
                planned(PlanDecision {
                    action: PlanAction::CreateIssues(vec![new_issue("follow-up work")]),
                    rationale: "spotted a gap".into(),
                    needs_human: false,
                }),
            );
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let (shipped, plan) = engine.drain().await.unwrap();
        assert_eq!(shipped, 1);
        // The forge grew by exactly the one created issue, and it carries the title.
        assert_eq!(forge.issue_count(), 2);
        assert!(forge.issue_titles().contains(&"follow-up work".to_string()));
        assert!(matches!(plan.unwrap().action, PlanAction::CreateIssues(_)));
    }

    /// A ready issue already scoped to a milestone by title, seeded directly so a test can
    /// control both list order and labels (which `FakeForge::create_issue` normalises).
    fn ready_in(id: u64, milestone: &str) -> Issue {
        Issue {
            milestone: Some(milestone.into()),
            state: IssueState::Open,
            ..ready(id)
        }
    }

    /// A milestone-scoped issue the selector must refuse, so a test can express "this
    /// milestone has work left, but none of it is workable".
    fn blocked_in(id: u64, milestone: &str) -> Issue {
        let mut issue = ready_in(id, milestone);
        issue.labels.push("status:needs-human".into());
        issue
    }

    fn floor_cfg() -> Config {
        let mut cfg = cfg();
        cfg.select.milestone_floor = true;
        cfg
    }

    /// Build an engine over `forge` with no scripted agent work. The floor tests exercise
    /// selection only, so the backend is never reached.
    fn selector_engine<'a>(
        cfg: &'a Config,
        forge: &'a FakeForge,
        backend: &'a FakeBackend,
    ) -> Engine<'a> {
        Engine::new(
            cfg,
            forge,
            backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn milestone_floor_prefers_the_earliest_open_milestone() {
        // The later milestone's issue is seeded first, so an unscoped selector would take
        // it. The floor must reach past list order to the earlier milestone's work.
        let cfg = floor_cfg();
        let forge = FakeForge::new(
            vec![ready_in(1, "v0.2"), ready_in(2, "v0.1")],
            CiState::Pass,
        );
        forge
            .create_milestone("v0.2", Some("2026-09-01"), "")
            .await
            .unwrap();
        forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(2));
    }

    #[tokio::test]
    async fn the_floor_costs_one_issue_fetch_however_many_milestones_there_are() {
        // The point of the floor being pure ordering. Every adapter implements selection as
        // one list fetch filtered in process, so ranking by asking per milestone would be M
        // identical fetches with M-1 discarded. Without this counter, a refactor that
        // reintroduces the per-milestone loop passes every behavioural test above.
        let cfg = floor_cfg();
        let forge = FakeForge::new(vec![ready_in(1, "v0.5")], CiState::Pass);
        for (title, due) in [
            ("v0.1", "2026-08-01"),
            ("v0.2", "2026-09-01"),
            ("v0.3", "2026-10-01"),
            ("v0.4", "2026-11-01"),
            ("v0.5", "2026-12-01"),
        ] {
            forge.create_milestone(title, Some(due), "").await.unwrap();
        }
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(forge.call_count("list_ready_issues"), 1);
        assert_eq!(forge.call_count("list_milestones"), 1);
    }

    #[tokio::test]
    async fn selection_with_the_floor_off_reads_no_milestones_at_all() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(forge.call_count("list_ready_issues"), 1);
        assert_eq!(forge.call_count("list_milestones"), 0);
    }

    #[tokio::test]
    async fn the_soft_floor_still_reaches_a_closed_milestones_leftover_work() {
        // Pins the behaviour the ordering test above does NOT cover. `milestone_floor_order`
        // drops closed milestones, but the final fallback is unscoped, so an issue left
        // behind in a closed milestone is still selectable. That is deliberate (never
        // starve), and it means "closed milestones are dropped" is a claim about the
        // ORDERING, not about what can be selected.
        let cfg = floor_cfg();
        let forge = FakeForge::new(vec![ready_in(1, "v0.1")], CiState::Pass);
        let closed = forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        forge.close_milestone(closed.id).await.unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(1));
    }

    #[tokio::test]
    async fn milestone_floor_falls_through_a_milestone_with_no_workable_issue() {
        // v0.1 still has an open child, but it is parked behind needs-human. The floor must
        // treat "no ready work" as drained and advance, or the loop stalls forever.
        let cfg = floor_cfg();
        let forge = FakeForge::new(
            vec![blocked_in(1, "v0.1"), ready_in(2, "v0.2")],
            CiState::Pass,
        );
        forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        forge
            .create_milestone("v0.2", Some("2026-09-01"), "")
            .await
            .unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(2));
    }

    #[tokio::test]
    async fn milestone_floor_order_prefers_an_open_milestone_over_a_closed_one() {
        // Closing v0.1 advances the floor even though its due date is still the earliest.
        let cfg = floor_cfg();
        let forge = FakeForge::new(
            vec![ready_in(1, "v0.1"), ready_in(2, "v0.2")],
            CiState::Pass,
        );
        let closed = forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        forge
            .create_milestone("v0.2", Some("2026-09-01"), "")
            .await
            .unwrap();
        forge.close_milestone(closed.id).await.unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(2));
    }

    #[tokio::test]
    async fn milestone_floor_is_soft_and_still_finds_unmilestoned_work() {
        // No open milestone has ready work, so the final unscoped probe must still pick up
        // an issue that belongs to no milestone at all. This is what stops the floor from
        // starving the drain.
        let cfg = floor_cfg();
        let forge = FakeForge::new(vec![blocked_in(1, "v0.1"), ready(2)], CiState::Pass);
        forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(2));
    }

    #[tokio::test]
    async fn milestone_floor_reports_no_work_when_there_is_none() {
        // Every probe misses. The floor must not invent work, or `run_one` would never
        // reach NoReadyWork and the drain would not terminate.
        let cfg = floor_cfg();
        let forge = FakeForge::new(vec![blocked_in(1, "v0.1")], CiState::Pass);
        forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        assert!(engine.select_ready_issue().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn explicit_milestone_scope_wins_over_the_floor() {
        // `select.milestone` is a hard answer to the same question, so the floor must not
        // reorder around it: v0.1 has ready work, but the pinned scope is v0.2.
        let mut cfg = floor_cfg();
        cfg.select.milestone = Some("v0.2".into());
        let forge = FakeForge::new(
            vec![ready_in(1, "v0.1"), ready_in(2, "v0.2")],
            CiState::Pass,
        );
        forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        forge
            .create_milestone("v0.2", Some("2026-09-01"), "")
            .await
            .unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(2));
    }

    #[tokio::test]
    async fn selection_order_is_unchanged_when_the_floor_is_off() {
        // The default. Same seeding as the "prefers earliest" test, opposite expectation,
        // so the two together pin the flag as the only thing that moves selection.
        let cfg = cfg();
        let forge = FakeForge::new(
            vec![ready_in(1, "v0.2"), ready_in(2, "v0.1")],
            CiState::Pass,
        );
        forge
            .create_milestone("v0.2", Some("2026-09-01"), "")
            .await
            .unwrap();
        forge
            .create_milestone("v0.1", Some("2026-08-01"), "")
            .await
            .unwrap();
        let backend = FakeBackend::new();
        let engine = selector_engine(&cfg, &forge, &backend);

        let got = engine.select_ready_issue().await.unwrap().unwrap();
        assert_eq!(got.id, IssueId(1));
    }

    /// A planner-proposed issue carrying placement hints.
    fn placed_issue(title: &str, milestone: Option<&str>, epic: Option<&str>) -> NewIssue {
        NewIssue {
            milestone: milestone.map(str::to_string),
            epic: epic.map(str::to_string),
            ..new_issue(title)
        }
    }

    /// Ship issue 1, then run the planner's `CreateIssues` decision through the engine.
    async fn drain_creating(forge: &FakeForge, cfg: &Config, list: Vec<NewIssue>) {
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review())
            .script(
                Role::Planner,
                planned(PlanDecision {
                    action: PlanAction::CreateIssues(list),
                    rationale: "spotted a gap".into(),
                    needs_human: false,
                }),
            );
        let engine = Engine::new(
            cfg,
            forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
        let (shipped, _) = engine.drain().await.unwrap();
        assert_eq!(shipped, 1);
    }

    #[tokio::test]
    async fn planner_files_a_created_issue_under_the_milestone_it_named() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let ms = forge.create_milestone("v0.1", None, "").await.unwrap();
        drain_creating(
            &forge,
            &cfg,
            vec![placed_issue("follow-up work", Some("v0.1"), None)],
        )
        .await;

        let children = forge.milestone_children(ms.id).await.unwrap();
        assert_eq!(
            children
                .iter()
                .map(|c| c.title.as_str())
                .collect::<Vec<_>>(),
            ["follow-up work"]
        );
    }

    #[tokio::test]
    async fn planner_milestone_hint_matches_a_title_case_insensitively() {
        // The planner retypes the title out of the snapshot; case drift must not silently
        // dump the issue at the top level.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let ms = forge.create_milestone("v0.1 MVP", None, "").await.unwrap();
        drain_creating(
            &forge,
            &cfg,
            vec![placed_issue("follow-up work", Some("v0.1 mvp"), None)],
        )
        .await;

        assert_eq!(forge.milestone_children(ms.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn planner_hint_naming_nothing_still_creates_the_issue_at_top_level() {
        // An unresolvable hint costs placement, never the issue: a human sees the work.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let ms = forge.create_milestone("v0.1", None, "").await.unwrap();
        drain_creating(
            &forge,
            &cfg,
            vec![placed_issue("follow-up work", Some("v9.9"), Some("nope"))],
        )
        .await;

        assert!(forge.issue_titles().contains(&"follow-up work".to_string()));
        assert!(forge.milestone_children(ms.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn planner_files_a_created_issue_under_the_epic_it_named() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let epic = forge.create_epic("Tracking rails", "").await.unwrap();
        drain_creating(
            &forge,
            &cfg,
            vec![placed_issue("follow-up work", None, Some("Tracking rails"))],
        )
        .await;

        let epics = forge.list_epics().await.unwrap();
        let updated = epics.iter().find(|e| e.id == epic.id).unwrap();
        assert_eq!(updated.children.len(), 1);
    }

    #[tokio::test]
    async fn planner_close_milestone_is_surfaced_not_executed() {
        // A milestone with an unfinished child must stay open even when the planner asks to
        // close it: CloseMilestone is surfaced (returned) but never auto-executed here.
        let cfg = cfg();
        let forge = FakeForge::new(vec![], CiState::Pass);
        let ms = forge.create_milestone("Phase 2", None, "").await.unwrap();
        let open_child = forge
            .create_issue(&new_issue("unfinished"), Some(ms.id), None)
            .await
            .unwrap();
        // Claim the milestone's child so it stays out of the ready pool (in-progress, not
        // done): the milestone has unfinished work and cannot be verifiably drained.
        let _guard = forge.claim(open_child.id).await.unwrap();
        // A separate standalone issue is what actually ships (no milestone, so the
        // verified-drain path cannot close anything on its own).
        let shippable = forge
            .create_issue(&new_issue("standalone"), None, None)
            .await
            .unwrap();
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(shippable.id.0))
            .script(Role::Reviewer, clean_review())
            .script(
                Role::Planner,
                planned(PlanDecision {
                    action: PlanAction::CloseMilestone("Phase 2".into()),
                    rationale: "looks done to me".into(),
                    needs_human: false,
                }),
            );
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let (shipped, plan) = engine.drain().await.unwrap();
        assert_eq!(shipped, 1);
        // The planner path did NOT close the milestone.
        assert_eq!(forge.milestone_state(ms.id), TrackState::Open);
        // The decision is surfaced to the caller for a human to action.
        assert_eq!(
            plan.unwrap().action,
            PlanAction::CloseMilestone("Phase 2".into())
        );
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

    #[tokio::test]
    async fn drain_emits_lifecycle_events() {
        use crate::events::EngineEvent;
        // Two ready issues, CI green so both ship.
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1), ready(2)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review())
            .script(Role::Implementer, ship_outcome(2))
            .script(Role::Reviewer, clean_review())
            .script(
                Role::Planner,
                planned(PlanDecision {
                    action: PlanAction::Stop,
                    rationale: "nothing left".into(),
                    needs_human: false,
                }),
            );
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = EngineHooks {
            sink: Some(tx),
            cancel: None,
            subsession: None,
        };
        let (shipped, _) = engine.drain_with(&hooks).await.unwrap();
        assert_eq!(shipped, 2);

        let mut evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            evs.push(ev);
        }
        assert_eq!(evs.first(), Some(&EngineEvent::DrainStarted));
        assert!(matches!(
            evs.last(),
            Some(EngineEvent::DrainComplete { shipped: 2 })
        ));
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, EngineEvent::IssueShipped { .. }))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn cancel_stops_after_current_issue() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1), ready(2)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review())
            .script(Role::Implementer, ship_outcome(2))
            .script(Role::Reviewer, clean_review());
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        // Set before the loop starts: cancel is observed at the top of the first
        // iteration, so nothing ships.
        let cancel = Arc::new(AtomicBool::new(true));
        let hooks = EngineHooks {
            sink: None,
            cancel: Some(cancel),
            subsession: None,
        };
        let (shipped, _) = engine.drain_with(&hooks).await.unwrap();
        assert_eq!(shipped, 0);
    }

    struct FakeCtx;
    #[async_trait::async_trait]
    impl crate::context::ContextProvider for FakeCtx {
        async fn ensure_ready(&self) {}
        fn mcp_servers(&self) -> Vec<crate::mcp::McpServer> {
            vec![crate::mcp::McpServer {
                name: "codegraph".into(),
                command: "codegraph".into(),
                args: vec!["serve".into(), "--mcp".into()],
            }]
        }
    }

    #[tokio::test]
    async fn engine_stamps_mcp_servers_from_context_provider() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let seen = backend.seen_mcp();
        let ctx = FakeCtx;
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap()
        .with_context(&ctx);

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::Shipped);

        let recorded = seen.lock().unwrap();
        assert!(
            recorded
                .iter()
                .any(|m| m.iter().any(|s| s.name == "codegraph")),
            "the backend should have received codegraph in mcp_servers"
        );
    }

    struct FakeConventions;
    impl crate::conventions::ConventionsProvider for FakeConventions {
        fn preamble_for(&self, role: Role, _worktree: &Path) -> Option<String> {
            match role {
                Role::Implementer | Role::Reviewer | Role::FixApplier => {
                    Some("PREAMBLE".to_string())
                }
                _ => None,
            }
        }
    }

    #[tokio::test]
    async fn engine_injects_the_conventions_preamble_for_the_implementer() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let seen = backend.seen_preamble();
        let conventions = FakeConventions;
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap()
        .with_conventions(&conventions);

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::Shipped);

        let recorded = seen.lock().unwrap();
        assert!(
            recorded
                .iter()
                .any(|p| p.as_deref() == Some("PREAMBLE")),
            "the backend should have received the conventions preamble"
        );
    }

    #[tokio::test]
    async fn engine_without_conventions_leaves_the_preamble_none() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let seen = backend.seen_preamble();
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        engine.run_one().await.unwrap();

        let recorded = seen.lock().unwrap();
        assert!(
            recorded.iter().all(|p| p.is_none()),
            "no provider means no preamble is ever injected"
        );
    }

    #[tokio::test]
    async fn engine_without_context_leaves_mcp_servers_empty() {
        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, clean_review());
        let seen = backend.seen_mcp();
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let outcome = engine.run_one().await.unwrap();
        assert_eq!(outcome, IterOutcome::Shipped);

        assert!(seen.lock().unwrap().iter().all(|m| m.is_empty()));
    }

    #[test]
    fn summary_for_implementer_ready_and_blocked() {
        let ready = ship_outcome(1);
        assert_eq!(
            subsession_summary(Role::Implementer, &Ok(ready)),
            ("ready to ship".to_string(), true)
        );
        let blocked = AgentOutcome {
            status: AgentStatus::Blocked,
            handoff: None,
            review: None,
            plan: None,
            summary: "x".into(),
            usage: Usage::default(),
            blocked_reason: Some("needs a device".into()),
        };
        assert_eq!(
            subsession_summary(Role::Implementer, &Ok(blocked)),
            ("blocked: needs a device".to_string(), false)
        );
    }

    #[tokio::test]
    async fn a_reviewer_that_wrote_no_report_is_shown_the_way_the_engine_treats_it() {
        // `run_stages` substitutes an empty Approve for a missing report and ships. The pane
        // used to call the same outcome an error, so an operator saw a red dot on a stage the
        // engine had approved and would file a bug against the engine. Asserted end to end
        // rather than on `subsession_summary` alone, so the two cannot drift apart again.
        let no_report = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: None,
            plan: None,
            summary: "x".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        assert_eq!(
            subsession_summary(Role::Reviewer, &Ok(no_report.clone())),
            ("approved (no report written)".to_string(), true)
        );

        let cfg = cfg();
        let forge = FakeForge::new(vec![ready(1)], CiState::Pass);
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, no_report);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = EngineHooks {
            sink: None,
            cancel: None,
            subsession: Some(tx),
        };
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();
        assert_eq!(
            engine.run_one_hooked(&hooks).await.unwrap(),
            IterOutcome::Shipped,
            "the engine ships on a missing report"
        );
        let mut reviewer_ok = false;
        while let Ok(ev) = rx.try_recv() {
            if let SubsessionEvent::Completed {
                role: Role::Reviewer,
                ok,
                ..
            } = ev
            {
                reviewer_ok = ok;
            }
        }
        assert!(reviewer_ok, "so the pane must not call that stage failed");
    }

    #[test]
    fn summary_for_reviewer_approve_and_request_changes() {
        let approve = clean_review();
        assert_eq!(
            subsession_summary(Role::Reviewer, &Ok(approve)),
            ("approved".to_string(), true)
        );
        // The finding count in the summary is scoped to Blocking/Major (what actually
        // gates), not the raw finding count: one Blocking and one Minor finding here
        // reports as "1 finding", not "2 findings".
        let changes = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![
                    Finding {
                        severity: Severity::Blocking,
                        file: "a.rs".into(),
                        line: None,
                        claim: "bug".into(),
                    },
                    Finding {
                        severity: Severity::Minor,
                        file: "b.rs".into(),
                        line: None,
                        claim: "nit".into(),
                    },
                ],
                verdict: Verdict::RequestChanges,
            }),
            plan: None,
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        assert_eq!(
            subsession_summary(Role::Reviewer, &Ok(changes)),
            ("changes needed (1 finding)".to_string(), false)
        );
    }

    #[test]
    fn summary_for_reviewer_gates_on_severity_not_verdict() {
        // A Major finding under an Approve verdict must still report as not-ok: the
        // summary gates on `has_blocking_or_major()`, the same predicate the verify loop
        // uses, not the advisory verdict field.
        let major_but_approved = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Major,
                    file: "a.rs".into(),
                    line: None,
                    claim: "wrong condition".into(),
                }],
                verdict: Verdict::Approve,
            }),
            plan: None,
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        assert_eq!(
            subsession_summary(Role::Reviewer, &Ok(major_but_approved)),
            ("changes needed (1 finding)".to_string(), false)
        );

        // A Minor-only report under RequestChanges must still report as approved: the
        // loop ships it as-is, so the summary must not say "changes needed".
        let minor_but_request_changes = AgentOutcome {
            status: AgentStatus::ReadyToShip,
            handoff: None,
            review: Some(ReviewReport {
                findings: vec![Finding {
                    severity: Severity::Minor,
                    file: "b.rs".into(),
                    line: None,
                    claim: "nit".into(),
                }],
                verdict: Verdict::RequestChanges,
            }),
            plan: None,
            summary: "minor".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        assert_eq!(
            subsession_summary(Role::Reviewer, &Ok(minor_but_request_changes)),
            ("approved".to_string(), true)
        );
    }

    #[test]
    fn summary_for_planner_and_error() {
        let planned_stop = planned(PlanDecision {
            action: PlanAction::Stop,
            rationale: "done".into(),
            needs_human: false,
        });
        assert_eq!(
            subsession_summary(Role::Planner, &Ok(planned_stop)),
            ("plan: stop".to_string(), true)
        );
        let err: Result<AgentOutcome> = Err(crate::traits::EngineError::Backend(
            "spawn claude: nope".into(),
        ));
        let (text, ok) = subsession_summary(Role::FixApplier, &err);
        assert!(!ok);
        assert!(text.starts_with("error:"), "got {text}");
    }

    #[tokio::test]
    async fn drain_emits_subsession_streams_per_role() {
        // One ready issue whose review requests changes, so all three worker roles run.
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
            plan: None,
            summary: "changes".into(),
            usage: Usage::default(),
            blocked_reason: None,
        };
        let backend = FakeBackend::new()
            .script(Role::Implementer, ship_outcome(1))
            .script(Role::Reviewer, dirty_review)
            .script(Role::FixApplier, ship_outcome(1))
            .script(Role::Reviewer, clean_review())
            .script(
                Role::Planner,
                planned(PlanDecision {
                    action: PlanAction::Stop,
                    rationale: "done".into(),
                    needs_human: false,
                }),
            );
        let engine = Engine::new(
            &cfg,
            &forge,
            &backend,
            Box::new(crate::workspace::NoopWorkspace::default()),
        )
        .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let hooks = EngineHooks {
            sink: None,
            cancel: None,
            subsession: Some(tx),
        };
        let (shipped, _) = engine.drain_with(&hooks).await.unwrap();
        assert_eq!(shipped, 1);

        let mut evs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            evs.push(ev);
        }

        // Each of the four roles produced a Started keyed to its issue+role. The three
        // worker roles run on issue 1; the Planner runs on the sentinel issue 0.
        let started_roles: Vec<(u64, Role)> = evs
            .iter()
            .filter_map(|e| match e {
                SubsessionEvent::Started { issue, role, .. } => Some((*issue, *role)),
                _ => None,
            })
            .collect();
        assert!(started_roles.contains(&(1, Role::Implementer)));
        assert!(started_roles.contains(&(1, Role::Reviewer)));
        assert!(started_roles.contains(&(1, Role::FixApplier)));
        assert!(started_roles.contains(&(0, Role::Planner)));

        // FakeBackend emits AgentEvent::Line("fake {Role}"), forwarded as a Delta.
        assert!(evs.iter().any(|e| matches!(
            e,
            SubsessionEvent::Delta { issue: 1, role: Role::Implementer, text } if text.contains("Implementer")
        )));

        // FakeBackend also emits AgentEvent::ToolUse("fake_tool"), forwarded as a Tool.
        assert!(evs.iter().any(|e| matches!(
            e,
            SubsessionEvent::Tool { role: Role::Implementer, name, .. } if name == "fake_tool"
        )));

        // The reviewer's Completed carries the changes-needed summary and ok=false.
        assert!(evs.iter().any(|e| matches!(
            e,
            SubsessionEvent::Completed { role: Role::Reviewer, ok: false, summary, .. }
                if summary.contains("changes needed")
        )));
        // The implementer's Completed is a successful ship.
        assert!(evs.iter().any(|e| matches!(
            e,
            SubsessionEvent::Completed {
                issue: 1,
                role: Role::Implementer,
                ok: true,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn drain_without_subsession_sink_still_ships() {
        // Default hooks have no subsession sink; the forwarder must still drain the backend
        // channel so nothing blocks, and the issue must ship exactly as before.
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
        assert_eq!(engine.run_one().await.unwrap(), IterOutcome::Shipped);
    }
}
