# Tutti: design (slice 2, the live adapters)

Date: 2026-07-11
Status: approved design, pre-implementation
Builds on: `docs/specs/2026-07-10-tutti-engine-design.md` (slice 1, the offline core)

## What this slice is

Slice 1 built `tutti-core`: the engine, the three seam traits, and in-memory fakes,
all green offline. This slice makes it real. It provides the first live
implementations of those traits plus the CLI that wires them, so `tutti run` drains a
real GitHub issue on a real repo end to end: pick the issue, run Claude to implement
and review it in an isolated worktree, open a PR, wait for CI, merge into the routed
integration branch, and record.

Scope (all chosen with the user): a Claude `AgentBackend`, a GitHub `Forge`, a
per-issue `WorktreeManager`, the `tutti` CLI binary with a PID lock, and the
`recover_stale` sweep deferred from slice 1. It branches off `slice1-core` (stacked),
so it does not block the slice-1 promotion PR.

## Decisions

- **File-based handoff.** A headless agent cannot return a Rust struct, so creative
  roles write `.tutti/handoff.json` and the reviewer writes `.tutti/review.json` in the
  worktree; the adapter reads and validates them into the existing `Handoff` /
  `ReviewReport` types. This mirrors SOTTO's `.git/engine-handoff.json`, is robust to
  agent chatter, and is identical across future backends (Codex, OpenCode).
- **The handoff protocol lives in the adapter, not the skills.** The `ClaudeBackend`
  always appends a fixed protocol postamble ("write your result as JSON matching this
  schema to `<abs path>`") to every creative/review run. Skills supply the working
  method (superpowers TDD, review); the backend owns the result contract. This keeps
  the protocol in one place and backend-agnostic.
- **Per-issue git worktrees.** A `WorktreeManager` creates
  `.worktrees/tutti-issue-N` on a fresh `feat/issue-N` branch for each issue and
  removes it after ship. True isolation, future parallelism, never dirties the main
  checkout. Matches the user's global `.worktrees/<branch>` convention.
- **CI stays offline-green.** Pure parsing/prompt-building logic is unit-tested
  against captured fixtures; `WorktreeManager` is tested against a local temp git repo
  (no network); the genuinely live paths (real `gh`, real `claude`) go in an opt-in
  integration tier gated behind a cargo feature so the required CI check stays
  hermetic.

## Component design

### 1. ClaudeBackend (impl `AgentBackend`)

New crate `tutti-backend-claude` (or module `backend/claude.rs`; a crate keeps the
`claude` process dependency out of `tutti-core`).

Flow of `run(task, worktree, events)`:

1. Build the prompt from `task`: the issue title/body/acceptance criteria, an
   instruction to invoke the `RolePlaybook.skills`, and the fixed protocol postamble
   naming the absolute output path (`<worktree>/.tutti/handoff.json` for
   Implementer/FixApplier, `<worktree>/.tutti/review.json` for Reviewer) and the JSON
   schema the file must match.
2. Spawn `claude -p <prompt> --model <task.model> --output-format stream-json
   --dangerously-skip-permissions` with `current_dir = worktree`.
3. Stream stdout line by line, parse each stream-json event, map to `AgentEvent`
   (`Line` / `ToolUse` / `Done`), and forward on the `events` channel.
4. On process exit:
   - Read the expected output file. If present and valid, deserialize into `Handoff`
     or `ReviewReport` and return `AgentOutcome { status: ReadyToShip, .. }`.
   - If absent (agent finished without emitting the artifact), return
     `AgentOutcome { status: Blocked, blocked_reason: Some("no handoff emitted"), .. }`
     (the slice-1 C2 contract: never panic).
   - If the stream contained a usage/limit/rate-limit marker, return
     `AgentOutcome { status: Error, .. }` so the CLI backs off.

Pure, unit-testable pieces (fixtures, no process): prompt construction, stream-json
line parsing, handoff/review JSON parsing, limit-marker detection.

### 2. Skill activation (the spike, done first)

The one real unknown: how `claude -p` activates a *named* skill in a headless run.
The first plan task is a spike that determines the exact mechanism (a prompt directive
naming the skill, `--append-system-prompt`, or reading the skill markdown and
prepending it) and records the finding. The adapter is built on whatever the spike
confirms. Fallback if no clean mechanism exists: resolve each skill ref to its markdown
and inject it as a prompt preamble (the "preamble fallback" from slice 1 s2.1).

### 3. GitHubForge (impl `Forge`)

New crate/module `tutti-forge-github`. Each trait method shells `gh` or `git`:

- `next_ready_issue`: `gh issue list --label <require_label> --state open --json
  number,title,body,labels,milestoneTitle`, drop any with a skip label, return the
  first. (Dependency-graph unblocking from SOTTO's `next_issue.py` is out of scope for
  this slice; a simple label filter matches the slice-1 `SelectFilter`.)
- `claim`: `gh issue edit N --add-label status:in-progress --remove-label
  status:ready`; if already in-progress, error so the engine reselects. Returns the
  slice-1 `ClaimGuard` token.
- `release`: the reverse label flip.
- `branch_exists`: `git ls-remote --exit-code --heads origin <branch>`.
- `create_branch`: `git push origin origin/<from>:refs/heads/<branch>` (ports SOTTO's
  stacked-branch creation).
- `open_pr`: `gh pr create --base --head --title --body-file`, parse the number/URL.
- `ci_status`: `gh pr checks <pr> --json state` mapped to `Pending | Pass | Fail`
  (the abstraction must not leak GitHub-only states; unknown states map to `Pending`).
- `merge`: `gh pr merge <pr> --squash --delete-branch`.
- `record`: relabel `status:done`, append the decision-log comment, best-effort unblock
  dependents.

`recover_stale` (new, deferred from slice 1): before selection, list `status:in-progress`
issues; for each, if no open PR references it and no live runner holds it, `release` it
back to `status:ready`. This is the crash-recovery net the slice-1 `ClaimGuard` could
not provide.

Pure, unit-testable pieces: `gh` JSON output parsing, `gh pr checks` state mapping,
branch-ref parsing. Live `gh` calls go in the integration tier.

### 4. WorktreeManager

Module in the CLI crate (it orchestrates git, not engine logic).

- `create(issue_number, base) -> Worktree { path, branch }`:
  `git worktree add .worktrees/tutti-issue-N -b feat/issue-N <base>`, where `base` is
  the routed integration branch if it exists on the remote, else the configured trunk.
- `remove(worktree)`: `git worktree remove --force <path>` after ship (and a sweep at
  startup that prunes leftover `tutti-issue-*` worktrees from a crashed run).

This forces one flow refinement in the engine (see below).

Tested against a real local temp git repo (create a repo, add a worktree, assert the
branch and path, remove it). No network, CI-safe.

### 5. Engine flow refinement

`RoutingStrategy::target_branch(issue)` is pure in the issue, so the live flow computes
the `BranchPlan` at the *start* of `run_claimed` (before implement), uses it to choose
the worktree base and create the worktree, runs the stages in that worktree, and still
overwrites `handoff.target` with the same plan before ship (unchanged guardrail). This
is a small, additive change to `tutti-core::engine`: thread a `WorktreeManager` and the
pre-computed `BranchPlan` into `run_claimed`. The offline tests keep passing because
the fake path can use a no-op worktree manager.

To keep `tutti-core` free of a git dependency, the worktree seam is a small trait
(`Workspace`) with a real git implementation in the CLI crate and a no-op fake in
`tutti-core::testing`.

### 6. tutti CLI binary

New crate `tutti-cli` producing the `tutti` binary.

- `tutti run [--config tutti.toml] [--loop <interval>]`: load and validate config,
  construct `GitHubForge` + `ClaudeBackend` + the routing strategy + a git `Workspace`,
  run `engine.drain()`. `--loop` re-drains on an interval; absent it, one drain and
  exit. Scheduling stays external (cron/launchd), same as SOTTO.
- A PID-aware lock (port SOTTO's mkdir-lock with pid-file self-heal) so two runs cannot
  race the same repo.
- Structured logs to stdout and a run log directory.

## Testing tiers

- **Hermetic (required CI):** all pure functions against captured fixtures, the engine
  loop against fakes (from slice 1), and `WorktreeManager` against a local temp git
  repo. No network, no `gh`, no `claude`.
- **Live integration (opt-in):** behind a cargo feature (e.g. `--features live` or a
  `TUTTI_LIVE=1` gate), end-to-end tests against a throwaway GitHub repo and real
  `claude`. Run on the self-hosted box, never in the hosted required check. This is the
  slice-2 analogue of SOTTO's `-m gpu` tier.

## Out of scope for this slice

- Backends other than Claude (Codex, OpenCode).
- Forges other than GitHub (`glab`, `tea`) - that is slice 3's forge-integration work
  for epics/milestones/roadmaps.
- Dependency-graph issue unblocking (SOTTO's `Depends on` resolution).
- The interactive UI (slice 4) and the new-project bootstrapper (slice 3).

## Open questions and risks

- **Skill activation mechanism** (the spike). Highest uncertainty; resolved first,
  with the preamble-injection fallback if needed.
- **stream-json schema stability.** The parser should tolerate unknown event types
  (map to a generic `Line`) so a `claude` update does not break the adapter.
- **`gh pr checks` output shape** across check types (Actions, external statuses); the
  `Pass/Fail/Pending` mapping must default unknown to `Pending` and be fixture-tested.
- **Worktree base when the integration branch does not yet exist:** branch off the
  configured trunk; the executor still creates the integration branch on first ship.
