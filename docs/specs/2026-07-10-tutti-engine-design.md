# Tutti: design (slice 1, the rails engine)

Date: 2026-07-10
Status: approved design, pre-implementation

## What Tutti is

Tutti is a standalone, cross-platform tool that drives multiple coding-agent
backends (Claude Code first, then Codex, OpenCode) through a strict,
forge-integrated development workflow. It generalizes the autonomous engine built
for the SOTTO project (`automation/`) into a reusable product: an opinionated
"on-rails" loop that turns tracked work (issues, epics, milestones) into reviewed,
merged code without a human babysitting each step.

The name is musical, a sibling to SOTTO (*sotto voce*): *tutti* means "all
instruments together." One conductor, many players, one score.

## How we got here (decisions)

- **Not Maestro, not a fork.** RunMaestro is a capable multi-backend orchestration
  workbench, but once Tutti owns its own UI, rails, forge layer, and onboarding, the
  only thing Maestro contributes is a normalized multi-backend session runner, which
  each agent's own headless CLI already provides (`claude -p`, `codex exec`,
  `opencode run`). Taking an Electron app as a driven dependency to avoid writing a
  few thin adapters is a bad trade. Maestro is dropped; Tutti owns a small
  `AgentBackend` interface instead.
- **Rust + Tauri.** Tutti is fundamentally a long-running supervisor daemon that
  spawns and babysits child processes, streams their output, runs a state machine,
  and later serves a desktop UI. It is I/O-bound orchestration, not compute. Rust
  gives exhaustive state-machine handling (the conductor must be more reliable than
  the agents it supervises), and Tauri is by far the leanest cross-platform desktop
  distribution story (a ~10MB binary vs Electron's ~150MB), with engine and UI in one
  language and one process. SOTTO already has Rust in-house (the iroh sidecar).
- **Prompts are skills, not baked-in strings.** Each role in the loop binds to a
  skill reference from config (superpowers skills where they fit, plus
  Tutti-authored skills). Skills are the primary cross-backend prompt interface;
  markdown-preamble injection is a fallback only for a backend that lacks skill
  support.
- **GitHub first** for the forge, behind a `Forge` trait so GitLab (`glab`) and
  Codeberg (`tea`) follow.

## The product decomposition and build order

Tutti is four subsystems (the agent-backend adapter layer folds into the first).
They are built in dependency order:

1. **Rails engine + agent-backend adapter** (this spec). The autonomous
   select -> implement -> review -> apply-fixes -> gate -> merge -> record -> plan
   loop, with Claude as the first backend. The differentiator everything else wraps.
2. **Forge integration.** GitHub-first, two-way read/write of issues, epics,
   milestones, roadmaps. Grows the `Forge` trait the engine already depends on.
3. **New-project bootstrapper.** The "new project" wizard: create the forge repo,
   seed issue labels, scaffold CI (test, lint, format, pre-commit, build,
   version-tag, release), run a deep brainstorm into a design/roadmap/epics/
   milestones/issues, choose a branching and release flow, set up and control the
   autoimplementer (play/pause/resume).
4. **Interactive UI (Tauri).** Chat with the orchestrator, drop into any subsession
   to chat with it, and view the tracking board. A wrapper over subsystems 1 to 3.

(Numbering above is the build order the user chose; the UI is last because it is a
pure wrapper over the runnable core.)

## Slice 1 design

### 1. Architecture and the stage pipeline

The engine is a crate (`tutti-core`) that knows nothing about UI. Everything welded
to one project becomes a trait with a default adapter:

| Welded to a single project | Slice-1 seam |
| --- | --- |
| `claude -p ... --dangerously-skip-permissions` | `AgentBackend` trait (Claude adapter first) |
| `gh`, one repo slug | `Forge` trait (GitHub adapter first) |
| `milestone/phase-N` stacking rules | `RoutingStrategy` trait (pluggable strategies) |
| `uv run ruff ... && pytest` | a `Gate`: a config-declared command list |

The drain loop runs a per-issue state machine and, after each issue, a planning
hook:

```
select -> implement -> review -> apply-fixes -> gate -> merge -> record -> plan
  ^        (creative    (fresh,   (creative,    (must   (mechanical,       |
  |         agent)      holistic  applies       be      no-permission)     |
  |                     reviewer) findings)     green)                     |
  +--------------------------------  next action  <------------------------+
        (planning agent proposes: next issue / new issues / close milestone / stop)
```

Preserved verbatim from the SOTTO engine because they are load-bearing:

- **State lives only in git + the forge.** Every stage re-derives from forge state,
  so a crash or usage-limit resumes cleanly and loses at most one issue's progress.
- **One issue per invocation**, drained in a loop with a safety cap.
- **The creative/mechanical split.** Agents *propose* via a typed handoff artifact; a
  separate no-permission executor performs the irreversible git and forge writes.
- **Label-based locking** (`status:ready -> in-progress -> done`) for multi-runner
  cooperation, plus a PID-aware single-runner lock.
- **The always-stop guardrail list** (money, hardware, secrets, publishing,
  the final merge to trunk). Enforced at three points (see section 4).

New in Tutti versus the SOTTO engine:

- **Review is a first-class stage with a fresh agent**, not folded into the
  implementer. Its output is a typed `ReviewReport`.
- **apply-fixes is a distinct creative pass** over the review findings, before the
  gate.
- **A planning hook** after each issue: a planner role proposes the next action.

### 2. Component contracts (the traits)

Each trait has one job, never calls the others or up into the engine, and has a fake
implementation used for testing.

#### AgentBackend

Runs one headless task and reports what happened. It does not touch git remotes, the
forge, or CI, which keeps every backend cheap and impossible to misuse.

```rust
trait AgentBackend {
    /// Run a headless task in `worktree`, streaming events out for the UI/logs.
    /// Returns when the agent exits. Never pushes, never merges.
    async fn run(&self, task: AgentTask, worktree: &Path,
                 events: Sender<AgentEvent>) -> Result<AgentOutcome>;
}
```

- `AgentTask` carries the resolved `RolePlaybook` (see 2.1), the issue/spec context,
  and the model to use.
- `AgentOutcome = { status: ReadyToShip | Blocked | Error, handoff: Option<Handoff>,
  summary, usage }`. `Blocked` carries a human-readable reason (hardware, secret,
  taste call).
- `events` is the streaming channel the UI subscribes to later; in slice 1 it drives
  logs. Designing it in now means the UI attaches with zero engine changes.

Claude adapter: `claude -p <prompt> --model ... --output-format stream-json`, parsed
into `AgentEvent`s. `FakeBackend` returns scripted outcomes for tests.

#### 2.1 Roles bind to skills

`AgentTask.role` does not select a baked-in prompt. The engine resolves each role to
a skill reference from config, and the backend activates that skill for the run.

```toml
[roles.implementer]
skills = ["superpowers:subagent-driven-development", "superpowers:test-driven-development"]

[roles.reviewer]
skills = ["superpowers:requesting-code-review"]

[roles.fix_applier]
skills = ["superpowers:receiving-code-review"]

[roles.planner]
skills = ["tutti:planning"]   # a skill shipped with Tutti
```

- The engine ships default mappings; a project's config overrides any role's skill
  list. This is how the loop is tuned to taste without touching the binary.
- Skills are the primary cross-backend prompt interface. Claude activates them
  natively; Codex and OpenCode (which can host installed Claude Code skills) map the
  same refs to their own activation. Markdown-preamble injection is a fallback only
  for a backend with no skill support.
- `RolePlaybook` (the resolved `{role -> skill refs}`) is what travels in
  `AgentTask`; each backend renders it its own way.

#### Forge

Everything issue/PR/CI, behind `gh` first. The engine speaks domain types; the
adapter translates to CLI invocations.

```rust
trait Forge {
    async fn next_ready_issue(&self, filter: &SelectFilter) -> Result<Option<Issue>>;
    async fn claim(&self, issue: &IssueId) -> Result<ClaimGuard>;   // label flip = the lock
    async fn open_pr(&self, pr: PrRequest) -> Result<PrHandle>;
    async fn ci_status(&self, pr: &PrHandle) -> Result<CiState>;    // Pending | Pass | Fail
    async fn merge(&self, pr: &PrHandle, how: MergeMode) -> Result<()>;
    async fn record(&self, issue: &IssueId, outcome: &ShipRecord) -> Result<()>;
    // labels, comments, decision-log append, dependent-unblock
}
```

`claim` returns a `ClaimGuard` (RAII): if a run dies mid-issue, `Drop` releases the
label lock so the next run re-selects (the equivalent of the SOTTO engine's
`recover_stale`). Slice 2 grows this trait for epics, milestones, and roadmaps;
slice 1 needs only the above.

#### RoutingStrategy

Decides one thing: given an issue, what branch its work targets and what happens on
merge. Nothing else.

```rust
trait RoutingStrategy {
    /// The integration branch this issue's work merges into (created if absent,
    /// stacked per the strategy's rules). NEVER returns the trunk.
    fn target_branch(&self, issue: &Issue, forge: &dyn Forge) -> Result<BranchPlan>;
    fn on_drained(&self, ctx: &DrainCtx) -> Result<NextPhase>;  // what "phase done" means
}
```

Built-in strategies: `PhaseStacking` (ports the SOTTO `milestone/phase-N` logic),
`VersionBranch` (`version/vX.Y`), `EpicBranch` (`epic/<slug>`), `Trunk` (feature
branch to a configured integration branch). The never-merge-to-trunk invariant lives
**above** the trait, in the executor, so a buggy strategy cannot violate the
guardrail.

#### Gate

A config-declared command list; all must exit 0 on a pristine tree. Data, not a
trait.

```toml
[gate]
commands = ["uv run ruff check .", "uv run pytest"]
working_dir = "server/orchestrator"
```

### 3. State model, handoff, and crash-safe resume

The only durable state is git + the forge. A small local store is a cache and UI
feed, never a source of truth for control flow. Delete it and a run rebuilds from
forge labels and branch state.

- **Per-issue state is derived, not stored.** The stage a crashed issue was in is
  re-derived from forge state: label `in-progress` and no PR -> restart from
  implement; PR open and CI pending -> resume at gate/merge; and so on. The
  `ClaimGuard` releases the lock on `Drop`.
- **The handoff artifact** is the typed contract between a creative stage and the
  mechanical executor (the SOTTO `.git/engine-handoff.json`, now typed), produced at
  two points:

  ```
  implement -> Handoff { issue, branch, target(BranchPlan), pr_title, pr_body, labels, decision_note }
  review    -> ReviewReport { findings: [{severity, file, line, claim}], verdict }
  apply     -> (amends the same branch; no new handoff)
  ```

  The executor consumes `Handoff` for the irreversible writes (push, open PR, wait
  CI, merge, relabel, decision-log, unblock) with no permission layer. It is total
  over the state machine.
- **The planning hook's output** is a separate typed artifact so it can never act by
  itself:

  ```
  plan -> PlanDecision { action: NextIssue | CreateIssues([..]) | CloseMilestone | Stop,
                         rationale, needs_human: bool }
  ```

  The engine executes the reversible/safe actions (`NextIssue`, `CreateIssues`).
  `CloseMilestone` and anything with `needs_human: true` is surfaced for a human,
  never auto-done.
- **Local store:** a SQLite file (run history, streamed agent events for the UI,
  cost/usage). Losing it costs history, not correctness.

### 4. Error handling, guardrail enforcement, and testing

**Failure philosophy: stop clean, never half-finish.** A failed stage stops the run
and leaves a reviewable artifact; it never fakes progress or force-pushes past a
problem.

- Implement/review/apply fails or the agent errors: no handoff, release the
  `ClaimGuard`, stop. The issue returns to `ready`.
- Gate red: a bounded number of fix passes, then stop with the issue left
  `in-progress` and a comment. Never merge red.
- CI red on the PR: leave the PR open, comment, stop.
- Usage/limit hit: detect, back off to the schedule, resume next firing.
- Merge/push fails: stop and surface; the work is committed on the branch, nothing
  lost.

Every stop is safe by construction because the only irreversible writes are in the
mechanical executor, which is total over the state machine.

**Guardrails have exactly three enforcement points**, so the always-stop list is
auditable:

1. **Selection filter:** `needs-human`-labeled issues are never selected.
2. **The executor's `BranchPlan` check:** refuses any target equal to the configured
   trunk. A buggy `RoutingStrategy` physically cannot merge to trunk.
3. **The planner gate:** `PlanDecision` actions are whitelisted; `CloseMilestone`,
   `needs_human: true`, and anything touching money/hardware/secrets/publish are
   surfaced, never executed.

**Testing: four fakes, whole loop green offline.** The trait boundaries exist so the
engine tests need no agent, no network, no git remote.

- `FakeBackend`: scripted `AgentOutcome`s (ready-to-ship, blocked, error,
  gate-fail-then-pass) to drive every branch of the state machine.
- `FakeForge`: an in-memory issue/PR/CI model; assert exact label transitions and
  merge targets.
- Real routing strategies tested against `FakeForge`.
- Gate: a temp script exiting 0 or 1.

The required CI check runs the full drain loop deterministically and offline.
Live-agent and live-`gh` tests are a separate, opt-in tier run only where
credentials exist.

## Out of scope for slice 1

- The forge integration beyond issues/PRs/CI (epics, milestones, roadmaps): slice 2.
- The new-project bootstrapper wizard: slice 3.
- The Tauri UI, orchestrator chat, subsession chat, tracking board: slice 4.
- Backends other than Claude: the `AgentBackend` trait is defined now; Codex and
  OpenCode adapters come later.
- Forges other than GitHub: the `Forge` trait is defined now; `glab` and `tea`
  adapters come later.

## Open questions and risks

- **Skill activation in headless runs.** Exact mechanism for forcing a specific skill
  to load in `claude -p` (and in Codex/OpenCode headless) needs verification during
  implementation. Fallback is preamble injection.
- **CI-status polling parity across forges.** `gh pr checks` semantics differ from
  `glab`/`tea`; the `CiState` abstraction must not leak GitHub assumptions.
- **Planner scope creep.** The planning hook is powerful; keep its action set small
  and whitelisted in slice 1 to avoid it becoming an unbounded agent.
