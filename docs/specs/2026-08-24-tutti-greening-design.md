# Tutti greening: drive the agent to a green gate (retrofit 2b)

Status: proposed
Date: 2026-08-24

## Goal

Retrofit (2a) installs an opinionated strict gate into an existing repo and reports an
honest baseline gap: the gate is usually red on legacy code (strict mypy, `clippy -D
warnings`, `tsconfig strict` flag things that predate them). 2b closes that gap the way
Tutti closes any gap: it drives Tutti's own coding agent to fix the code until the gate
passes. Tutti eats its own dog food.

`tutti green [path]` (and `tutti retrofit --then-green`) runs the repo's gate; where it is
red, it green-ups the code in an **isolated git worktree on a throwaway branch**, looping
gate -> agent fix -> gate until the gate passes or a cap is hit, then **opens a PR** with
the greening commit. A polyglot repo greens each language **in parallel**, one branch and
one PR per language. A run that stops short leaves its branch so a later run **resumes**
from it.

## Decisions (settled in brainstorming)

- **Landing:** an isolated git worktree on a throwaway branch (`green/<label>`), one
  greening commit, then a PR. Not in-place; not the forge-coupled `run_one` drain.
- **Cadence:** run iterations autonomously up to `--max-iters` (default 5). Stop early on
  green. On green: anti-cheat check -> commit -> PR. On exhaustion: leave the branch at its
  best state and report the still-failing gate log; no PR.
- **Multi-language, parallel:** discover gate targets and green them concurrently, each on
  its own branch, each opening **its own PR** (one PR per language). A failing target does
  not block the others.
- **Forge required:** opening a PR is the point, so `tutti green` requires a configured
  forge (and therefore a `tutti.toml`). It errors clearly when none is configured.
- **Resume:** if a target's greening branch already exists, continue from it (re-run the
  gate on its state and keep iterating) rather than restarting from trunk. `--fresh` forces
  a clean restart.
- **Anti-cheat = constrain + verify:** the greener playbook forbids cheating, and a
  deterministic post-check on the greening diff **hard-rejects** a target that edited the
  gate definition (`scripts/check.sh`, `.github/workflows/*`) and **flags** (still opens the
  PR, with warnings) added suppressions, deleted tests, and linter/type-config edits.

## The greening unit: a gate target

Greening operates on **gate targets**, not directly on "languages". A target is:

```rust
pub struct GateTarget {
    pub label: String, // friendly name for the branch, e.g. "python" (from detect_languages)
    pub gate: Gate,    // Gate { commands, working_dir }; working_dir is the target's dir
}
```

The target's directory lives in `gate.working_dir` (relative to the repo root; empty / `.`
for a single-language repo), so there is one source of truth for where the target lives and
what runs there.

**Discovery.** Each `scripts/check.sh` found at the repo root and one level of
subdirectories is one target. A single-language repo has a root `scripts/check.sh` -> one
target (the 2a case). A polyglot monorepo (the sandbox's `py/ rs/ ts/ go/` shape, once each
subdir carries its own `scripts/check.sh`) -> one target per subdir. The `label` comes from
running `detect_languages` (2a) against the target's `gate.working_dir`, falling back to the
directory name. The gate command for a discovered target is the canonical
`["bash scripts/check.sh"]` with `gate.working_dir` set to that directory.

**Config override.** When `tutti.toml` defines `[gate]` and the repo is single-target (one
root gate), that configured command list is used verbatim instead of the canonical default,
so a repo whose real gate is not `bash scripts/check.sh` still greens. `--gate "<cmd>"`
overrides for a one-off. (A per-target gate override is out of scope; a polyglot repo uses
the per-subdir `scripts/check.sh` convention.)

## Command surface

```
tutti green [PATH]                 # discover targets, green each in parallel, open a PR per target
tutti green --max-iters N [PATH]   # cap per-target iterations (default 5)
tutti green --fresh [PATH]         # discard any existing greening branch, restart from trunk
tutti green --base <branch> [PATH] # PR base (default: config integration_branch)
tutti green --gate "<cmd>" [PATH]  # override the gate command (single-target repos)
tutti retrofit --then-green [PATH] # chain: retrofit apply, then green (also needs a forge)
```

`tutti green` reads `tutti.toml` for `[forge]` (+ login), `[model]`, `[gate]`, and
`integration_branch`, exactly as `tutti run` does. It **requires** `tutti.toml` and a
forge; absent either, it errors with a clear message (opening a PR is the deliverable).
`retrofit --then-green` therefore also requires a forge; plain `retrofit` does not.

## Architecture

Four units, all over existing seams so the orchestration is hermetic and fake-testable.

### 1. `Role::Greener` (`tutti-core/src/message.rs` + `tutti-backend-claude/src/prompt.rs`)

A new `Role` variant with one prompt arm:

> "Make the repository's gate pass by fixing the underlying code. Do NOT suppress errors
> (`# type: ignore`, `# noqa`, `#[allow(...)]`, `eslint-disable`), weaken the gate
> configuration, or delete tests to make it pass. Fix the root cause."

The prompt already includes `issue.title` and `issue.body`; the synthetic task carries the
current failing gate log in the body, so no other backend change is needed. `subsession_summary`
gains the `Greener` arm.

### 2. The orchestration (`tutti-core/src/greening.rs`)

Pure over the `AgentBackend`, `Gate`, `Forge`, and `Workspace` traits. No `claude`, git, or
network in a unit test.

```rust
pub struct GreenOptions { pub max_iters: u32, pub fresh: bool, pub base: String, pub model: String }

pub struct GreenTargetResult {
    pub label: String,
    pub branch: String,
    pub outcome: GreenOutcome, // AlreadyGreen | Greened { iters, pr } | Exhausted { iters, gate_log } | Rejected { reason } | Error(String)
}

/// Green one target: worktree (resume or fresh) -> loop -> anti-cheat -> commit -> PR.
async fn green_target(
    target: &GateTarget, opts: &GreenOptions,
    backend: &dyn AgentBackend, workspace: &dyn Workspace, forge: &dyn Forge,
) -> GreenTargetResult;

/// Discover targets, green them concurrently (bounded), collect results.
pub async fn green_all(
    repo: &Path, targets: Vec<GateTarget>, opts: &GreenOptions,
    backend: &dyn AgentBackend, workspace: &dyn Workspace, forge: &dyn Forge,
) -> Vec<GreenTargetResult>;
```

Per-target flow:

1. **Worktree.** `green/<label>` under `.worktrees/`. If the branch exists and `!fresh`, add
   the worktree onto the existing branch (resume); else create it fresh from `config.trunk`
   (where the un-green code lives). The PR later targets `opts.base` (the integration branch
   by default), never trunk directly. (A named-worktree helper, see unit 3.)
2. **Baseline.** Run the target's gate in the worktree. Green already -> `AlreadyGreen`, no
   PR.
3. **Loop** `1..=max_iters`: build a synthetic `AgentTask` (`Role::Greener`, `issue.title =
   "Green up the <label> gate"`, `issue.body =` the current `GateOutcome.log` plus the
   constraint text), `backend.run(task, worktree, tx)`, then re-run the gate. Break on green.
4. **On green:** run the anti-cheat check (unit 4) over the worktree's diff vs base.
   - Rejected -> `Rejected { reason }`: no commit, no PR, branch left, reported loud.
   - Otherwise commit (`Workspace::commit_all`), `forge.push_branch`, `forge.open_pr` into
     `base` -> `Greened { iters, pr }` (with any non-fatal flags attached to the report).
5. **On exhaustion:** `Exhausted { iters, gate_log }`, branch left for resume, no PR.

Concurrency: `green_all` runs `green_target` for each target with a bounded fan-out (all at
once for a handful of targets; a small semaphore cap guards a pathological repo). Each result
is independent; the CLI prints a combined summary.

### 3. Named worktree (`tutti-git`)

`GitWorkspace` today derives its path/branch from an `IssueId` (`feat/issue-N` under
`.worktrees/`). Greening needs `green/<label>`, so `tutti-git` gains a named-worktree
create that takes an explicit branch name and base, plus a resume-aware variant (add a
worktree onto an existing branch without resetting it). Kept minimal and behind the same
`Workspace`-style shape the orchestration consumes so the loop stays trait-testable.

### 4. Anti-cheat check (`tutti-core/src/greening.rs`, pure)

`fn audit_green_diff(changed_files: &[PathBuf], diff: &str) -> GreenAudit` where

```rust
pub struct GreenAudit { pub reject: Option<String>, pub flags: Vec<String> }
```

- **Reject** (Some) if any changed path is `scripts/check.sh` or under
  `.github/workflows/`. These are the gate definition; editing them to pass is cheating.
- **Flag** (never blocks the PR, surfaced in the report): count of added suppression
  comments (`# type: ignore`, `# noqa`, `#[allow(`, `eslint-disable`) beyond zero; any
  deleted test file (path matching `test_*.py` / `*_test.go` / `*.test.ts` / `tests/`);
  any change to `pyproject.toml` / `tsconfig.json` / `package.json` / `Cargo.toml` /
  `go.mod` (could be a legitimate dependency add or a strictness-loosening cheat, so the
  reviewer decides).

Pure over the diff text + file list, unit-tested.

### 5. CLI (`tutti-cli/src/green.rs`)

Loads `tutti.toml`, builds the forge + `ClaudeBackend` + `GitWorkspace` via the existing
`wire::build`, discovers targets, calls `green_all`, and prints per-target progress
(`SubsessionEvent`s already flow from `AgentBackend`) and a combined final report (branch,
iters, PR url or the red gate log, plus any flags). `retrofit --then-green` calls retrofit's
apply path then hands off to the same `green_all`.

## Data flow (polyglot repo, happy path)

1. `tutti green .` in a repo with `py/scripts/check.sh` and `rs/scripts/check.sh`.
2. Discover two targets: `{python, py/, bash scripts/check.sh}`, `{rust, rs/, ...}`.
3. Concurrently: worktree `green/python` and `green/rust`; each loops gate -> greener -> gate.
4. Both reach green within the cap; each passes the anti-cheat check.
5. Commit each, push, open PR `green/python -> <integration>` and `green/rust -> <integration>`.
6. Report: two PR urls, iteration counts, no flags.

## Error handling

- **No `tutti.toml` / no forge:** `tutti green` errors before doing any work.
- **A target exhausts the cap:** reported red with its gate log, branch left for `--fresh`-less
  resume; other targets are unaffected.
- **Anti-cheat reject:** that target opens no PR and is reported as rejected; the branch is
  left so the operator can inspect what the agent did.
- **Backend/git/forge error on one target:** captured as `Error` in that target's result;
  `green_all` still returns the other targets' results (one failure never aborts the batch).
- **Worktree cleanup:** each target removes its worktree checkout on every terminal path
  (the branch persists; only the `.worktrees/<...>` checkout is torn down), mirroring the
  engine's `run_claimed`.

## Testing

- **Hermetic (`tutti-core`):**
  - `green_target` over a `FakeBackend` scripted red->green (asserts commit + `open_pr`),
    never-green (asserts `Exhausted`, no `open_pr`), and already-green (no backend call, no
    PR), with a fake `Gate` and `FakeForge`.
  - Anti-cheat reject path: a `FakeBackend` whose diff touches `scripts/check.sh` -> the
    target is `Rejected`, `FakeForge` never sees `open_pr`.
  - `audit_green_diff` unit tests: reject on `check.sh`/CI; flags for suppressions, deleted
    tests, config edits; clean diff -> no reject, no flags.
  - `green_all` collects mixed results (one greened, one exhausted, one rejected) without
    one aborting the batch.
  - Target discovery: root-only, per-subdir, and the `[gate]`/`--gate` override.
  - Resume: an existing `green/<label>` branch continues rather than restarting (fake
    workspace records which path was taken).
- **`tutti-cli`:** config/forge required-error; `--then-green` chaining; `--fresh`/
  `--max-iters` plumbing.
- **Live (`#[ignore]`, needs `claude` + a toolchain + a throwaway forge repo):** a
  genuinely-fixable dirty repo greens end-to-end and opens a real PR; a repo where the only
  way to "pass" is to edit `check.sh` is caught by the anti-cheat check. Manual, out of the
  required gate (mirrors the 2a live tests).

## Out of scope

- Auto-**merging** the greening PRs (the operator reviews and merges).
- Tuning the model or per-language fix strategies.
- A per-target gate override beyond the per-subdir `scripts/check.sh` convention and the
  single-target `[gate]`/`--gate`.
- Greening languages that 2a does not yet scaffold a gate for.
