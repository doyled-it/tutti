# Tutti retrofit: tutti-fy an existing repo

Status: proposed
Date: 2026-08-15

## Goal

Point Tutti at an existing repo and have it impose the same opinionated, agent-friendly
shape the create flow imposes on a fresh one (a real CI, formatters and linters as gates,
one canonical `scripts/check.sh` gate, `AGENTS.md`), **adding only what is missing and
never silently clobbering what is there**. Where the create flow starts from an empty
directory and a green placeholder, retrofit starts from real, pre-existing code that was
never held to these standards, so the two flows diverge in three ways that shape this
whole design:

1. Retrofit **detects** the language instead of asking the user to pick a stack.
2. Retrofit **must not** inject placeholder source or tests into a repo that already has
   real code.
3. Retrofit's gate, run against legacy code, will very likely be **red on first run**
   (strict mypy, `clippy -D warnings`, `tsconfig strict` flag things that predate them),
   so "green on first CI" (a create-time guarantee about placeholder code) is not a
   promise retrofit can honestly make.

The answer to (3) is the payoff, and it is on-brand: Tutti is an agent engine, so the
wall of errors retrofit surfaces is not homework dumped on the user, it is exactly the
kind of work Tutti exists to drive. Retrofit installs the strict rails and reports the
gap; then Tutti's own agent closes the gap in a supervised session. Tutti eats its own
dog food.

## Scope

This design covers **single-language-at-root** repos (the common case: one `Cargo.toml`,
or one `pyproject.toml`, at the repo root). If retrofit finds markers for more than one
language at the root, it reports them and asks the user to pick the primary one rather
than guessing. A polyglot monorepo (like `~/projects/tutti-live-sandbox`, with `py/ rs/
ts/ go/` subtrees under one composed `scripts/check.sh` with per-subdir `working_dir`) is
an explicit future extension, out of scope here.

The work is **phased**:

- **2a (this slice's core):** detect the language, install the strict rails (additive,
  no-clobber), merge our opinionated config into the repo's existing config files behind a
  mandatory diff-preview, run the installed gate once, and print an honest baseline report.
  Shipped alone, this is already useful: a repo that was not Tutti-ready becomes Tutti-ready
  with a real gate, real CI, and `AGENTS.md`.
- **2b:** the supervised **greening session** that takes retrofit's baseline gap and drives
  Tutti's own agent to green.

## Decisions (settled)

- **Opinions stay declarative data.** Retrofit reuses the `StackProfile` values that
  create already uses (`crates/tutti-app-core/src/scaffold.rs`). No second source of truth
  for what the opinionated Python/Rust/TypeScript/Go shape is.
- **Retrofit may edit hand-authored config files (merge), not only add new ones.** This is
  the one place retrofit departs from the create emitter's pure no-clobber contract, and it
  is gated by two invariants: a mandatory diff-preview-then-confirm before any byte is
  written, and format-preserving edits (comments and layout survive).
- **Only Python and TypeScript need a config *merge*.** Rust (`cargo fmt`/`clippy`/`test`)
  and Go (`gofmt`/`vet`/`test`) gates run entirely on defaults, so their retrofit is pure
  additive-new-files with no config to merge. That bounds the risky surface to two
  ecosystems.
- **Full opinion, honest gap report.** Retrofit installs the same strict gate create uses
  and does not soften it. Getting to green is the greening session's job (2b), not a reason
  to water down the rails.
- **CLI-first surface.** The hermetic core lives in `tutti-app-core`; the first surface is a
  `tutti-cli` `retrofit` subcommand. A Tauri wizard on the same core is a thin follow-up.

## The opinion being installed

Identical to the create-flow profiles, so the tables below are what already ships in
`scaffold.rs`. What differs per language is only which of those files retrofit **adds**
(new, no-clobber) versus **merges** (into an existing config) versus **never emits**
(placeholder source).

| Language | New files (additive) | Merge targets | Never emitted (Sample) |
|---|---|---|---|
| Python | `scripts/check.sh`, `.github/workflows/ci.yml`, `AGENTS.md`, `.python-version`, `.gitignore`* | `pyproject.toml` (`[tool.ruff]`, `[tool.mypy] strict`, `[tool.pytest.ini_options]`, `[dependency-groups] dev`) | `src/<pkg>/__init__.py`, `src/<pkg>/core.py`, `tests/test_core.py` |
| Rust | `scripts/check.sh`, `.github/workflows/ci.yml`, `AGENTS.md`, `.gitignore`* | (none) | `src/lib.rs` |
| TypeScript | `scripts/check.sh`, `.github/workflows/ci.yml`, `AGENTS.md`, `.gitignore`* | `tsconfig.json` (strict flags), `package.json` (dev deps + `check` script) | `src/index.ts`, `src/index.test.ts` |
| Go | `scripts/check.sh`, `.github/workflows/ci.yml`, `AGENTS.md`, `.gitignore`* | (none) | `<pkg>.go`, `<pkg>_test.go` |

`*` `.gitignore` is an **append-merge**, not a whole-file add: retrofit appends only the
ignore lines that are missing, preserving everything already there. If a New file already
exists (e.g. the repo has its own `.github/workflows/ci.yml`), retrofit **skips it and
reports it**; it never merges CI YAML (too fragile) and never overwrites it.

`Cargo.toml` and `go.mod` are the Rust/Go **detection markers**, so they always exist when
that language is detected. They carry no merge spec (the gates need no config), so retrofit
leaves them untouched; they appear in the plan's `already` list, never `adds` or `merges`.

## Architecture

Four units, each independently testable, mirroring the create-flow decomposition.

### 1. File roles on the profile (`scaffold.rs`)

`ScaffoldFile` gains a role so the same `StackProfile` can serve both flows:

```rust
pub enum FileRole {
    Sample,   // placeholder source/tests: create-only, retrofit never emits
    Tooling,  // new additive files: check.sh, ci.yml, AGENTS.md, .python-version
    Config,   // a merge target: pyproject.toml / tsconfig.json / package.json
}
```

Each `ScaffoldFile` carries a `role: FileRole`. The `.gitignore` file is tagged `Tooling`
but flagged for append-merge (see below). A `Config` file may additionally name a **merge
spec** (which keys/sections to ensure); a `Config` file with no merge spec (`Cargo.toml`,
`go.mod`, the detection markers) is left untouched by retrofit. This is the core
data-model change; it is backward-compatible with create, which emits all roles.

The existing create emitter (`scaffold`) is unchanged in behavior: it writes every role,
no-clobber. Retrofit is a **different consumer** of the same profile that filters by role.

### 2. Language detection (`tutti-app-core/src/retrofit.rs`)

`fn detect_languages(dir: &Path) -> Vec<StackId>` maps marker files to languages:

| Marker at repo root | Language |
|---|---|
| `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt` | `python` |
| `Cargo.toml` | `rust` |
| `package.json`, `tsconfig.json` | `typescript` |
| `go.mod` | `go` |

Pure and unit-tested against fixture directories. Zero markers -> retrofit reports "no
recognized language" and stops. More than one -> the flow surfaces all and asks the user to
pick one (single-language scope).

### 3. Plan -> preview -> apply (`retrofit.rs`)

`fn plan_retrofit(dir, profile, ctx) -> RetrofitPlan` computes, without touching disk:

```rust
pub struct RetrofitPlan {
    pub adds: Vec<ScaffoldFile>,       // Tooling files that do not exist yet
    pub merges: Vec<ConfigMerge>,      // Config files that exist and need edits (with a diff)
    pub gitignore_append: Vec<String>, // ignore lines missing from an existing .gitignore
    pub skipped: Vec<SkipReason>,      // Tooling files that already exist (e.g. their CI)
    pub already: Vec<PathBuf>,         // nothing to do (idempotent no-ops)
}
```

A `ConfigMerge` carries the target path, the computed new contents, and a **unified diff**
`old -> new` for the preview. `plan_retrofit` is pure over the filesystem read; it writes
nothing.

`fn apply_retrofit(dir, &RetrofitPlan) -> RetrofitReport` performs the writes: create the
`adds`, append the `gitignore_append`, and write each merged `Config` file. **`apply` is
only ever called after the operator confirms the plan** (the CLI enforces this; see the
surface section). Idempotent: a second `plan_retrofit` on an already-retrofitted repo yields
empty `adds`/`merges` and a populated `already`.

### 4. Config mergers (`retrofit.rs`, one per format)

The only merge targets are TOML (`pyproject.toml`) and JSON (`tsconfig.json`,
`package.json`).

- **TOML** uses `toml_edit` (already a dependency of `tutti-app-core`), which preserves
  comments and formatting and supports surgical inserts. `merge_pyproject` ensures the
  `[tool.ruff]`, `[tool.ruff.lint]`, `[tool.mypy]`, `[tool.pytest.ini_options]`, and
  `[dependency-groups]` sections exist with our opinionated values.
- **JSON** (`tsconfig.json`, `package.json`) is edited via `serde_json` (`package.json` is
  plain JSON; `tsconfig.json` is JSONC, so retrofit strips comments to parse and, if the
  original had comments, reports that the file was reformatted). `merge_tsconfig` ensures
  the strict compiler flags; `merge_package_json` ensures the `typescript` + `bun-types`
  dev dependencies and the `check` script.

**Conflict policy (the crux of merge):**

- **Additive where absent.** A section or key we opine on that the user has not set is
  added at our value.
- **Enforce only the opinion-defining strictness keys.** `mypy strict = true`,
  `tsconfig strict`/`noUncheckedIndexedAccess = true`, and the ruff lint selection are set
  to our values even if the user had a weaker value, because those keys *are* the opinion.
- **Preserve everything else byte-for-byte.** Cosmetic or non-load-bearing user values
  (e.g. `ruff line-length = 100`) are left untouched. No user content is removed or
  reordered.
- **The diff-preview is the safety valve.** Because every enforced change appears in the
  confirmed diff, the operator always sees and approves the exact edits before they land,
  and the greening session (2b) cleans up any fallout from enforcing strictness.

### 5. Baseline gate report (2a deliverable)

After `apply`, retrofit constructs the profile's `Gate` (`["bash scripts/check.sh"]`) and
runs it once via the existing `tutti_core::gate::Gate::run(dir)`, capturing
`GateOutcome { passed, log }`. It prints an **honest baseline**, e.g.:

```
Gate installed. Current state:
  ruff:          12 issues
  mypy --strict: 47 errors
  pytest:        passing
Getting to green is your migration. Run `tutti retrofit --then-green` to have
Tutti's agent knock it out.
```

The gate is not softened. A green baseline (nothing to fix) is reported as such.

### 6. Greening session (2b)

Retrofit's baseline gap becomes a **synthetic `Issue`**: title "Green up the strict gate",
body = the failing `GateOutcome.log`. That issue is run through the existing
`tutti_core::traits::AgentBackend::run(task, worktree, events)` machinery with a
fix-oriented `RolePlaybook` and a goal of "make `bash scripts/check.sh` exit 0", inside a
git worktree. The engine's existing gate-run loop verifies green; diffs surface at the
existing review checkpoints; the run produces one greening commit/PR. **No `AgentBackend`
or `Gate` trait change is needed** (`AgentTask.issue` is a plain `Issue`, so a synthesized
one is the only seam). This is why 2b is a modest addition rather than an engine rewrite.

## Surface

The hermetic core (detection, plan, merge, report) lives in `tutti-app-core` and is exposed
first through a **`tutti-cli` `retrofit` subcommand**:

```
tutti retrofit [PATH]           # detect, print the plan, confirm, apply, run gate, report
tutti retrofit --dry-run [PATH] # print the plan (adds, merges-with-diff, skips) and stop
tutti retrofit --yes [PATH]     # apply without the interactive confirm (CI/scripts)
tutti retrofit --then-green [PATH]  # 2b: after applying, launch the greening session
```

The default (no flag) prints the plan and requires an interactive "apply? [y/N]" before
calling `apply_retrofit`. `--dry-run` is the plan-only path; `--yes` is the non-interactive
path. The `retrofit` subcommand is added to the existing clap `Cmd` enum alongside `Run`.

A Tauri command + wizard panel on the same core is a follow-up; the core is written to be
surface-agnostic so the app can reuse it verbatim.

## Data flow (retrofit, single language)

1. `tutti retrofit ~/code/legacy-app`.
2. `detect_languages` reads the markers -> `python`.
3. `plan_retrofit` computes the `RetrofitPlan`: adds `scripts/check.sh`, `ci.yml`,
   `AGENTS.md`, `.python-version`; merges `pyproject.toml` (with a diff); appends two lines
   to `.gitignore`; skips nothing.
4. The CLI prints adds, the merge diff, and the gitignore append, then asks to apply.
5. On confirm, `apply_retrofit` writes them.
6. Retrofit runs `bash scripts/check.sh` and prints the baseline (e.g. mypy 47 errors).
7. If `--then-green`, the greening session (2b) drives the agent to close that gap.

Retrofit deliberately does **not** write `tutti.toml`, seed status labels, or create the
integration branch: those are the existing create/onboarding flow's job, and a repo already
under Tutti has them. Retrofit is strictly about installing the opinionated tooling. (If a
future "convert this repo" flow wants to also do the config/labels/branch seeding, it
composes retrofit with the existing onboarding, rather than retrofit duplicating it.)

## Error handling

- **No-clobber on Tooling files.** An existing `ci.yml`/`AGENTS.md`/`check.sh` is skipped
  and reported, never overwritten (create's emitter guarantee, reused).
- **Merge is confirmed, never silent.** `apply_retrofit` runs only after the operator
  approves the diff. `--yes` is the explicit opt-out for automation.
- **Format-preserving merge.** `toml_edit` keeps comments/layout; a JSONC `tsconfig.json`
  with comments is reported as reformatted so the change is never a surprise.
- **Idempotent re-run.** A second retrofit detects its own keys/files and produces an empty
  plan (`already` populated), reporting "already Tutti-ready".
- **Unrecognized or multi-language repo.** Zero markers -> stop with a clear message.
  Multiple -> ask the user to pick one (single-language scope).
- **Recoverability.** Retrofit operates on a git repo, so every merge is recoverable via
  `git`; the confirm + preview is the primary guard, git is the backstop.

## Testing

- **Hermetic unit tests (`tutti-app-core`):**
  - `detect_languages` against marker fixtures (each language, none, multiple).
  - `plan_retrofit`: adds computed correctly; a pre-existing `ci.yml` lands in `skipped`; a
    pre-existing `pyproject.toml` produces a `merge` not an add.
  - Config mergers via `toml_edit` golden tests: `pyproject.toml` with and without an
    existing `[tool.ruff]`; comment preservation; **idempotency** (merge twice = no second
    diff); enforce-strict vs preserve-cosmetic.
  - `.gitignore` append-merge (only missing lines added, order preserved).
  - `FileRole` filtering: retrofit's file set excludes every `Sample` file.
- **Live retrofit gate test (`#[ignore]`d, mirrors `*_scaffold_passes_its_own_gate`):**
  - Retrofit a **clean** fixture repo (valid code, no tooling) -> run the emitted gate ->
    assert green (baseline has nothing to fix).
  - Retrofit a **dirty** fixture (a deliberate type error) -> assert the baseline report
    flags it (gate red, log names the error).
  - Run with `env -C /Users/mdoyle/projects/tutti cargo test -p tutti-app-core -- --ignored`.
- **CLI test:** `--dry-run` prints a plan and writes nothing; the confirm path calls
  `apply` exactly once.
- Live end-to-end (a retrofitted real repo's CI passing on GitHub, and the 2b greening
  session against a real repo) is `test:manual` / behind `--features live`, out of the
  hermetic gate.

## Out of scope (explicit follow-ups)

- **Polyglot / monorepo retrofit** (multiple languages, a composed `scripts/check.sh` with
  per-subdir `working_dir`, the sandbox's own shape).
- **Config-merge for Rust/Go** beyond adding absent files (their gates need no config, so
  there is nothing to merge today).
- **Merging an existing CI workflow.** Retrofit adds its `ci.yml` only when absent and
  otherwise reports; it never edits the user's CI YAML.
- **The Tauri wizard surface.** CLI-first; the app panel reuses the same core later.
- **Also seeding `tutti.toml` / status labels / the integration branch** as part of
  retrofit. That belongs to the existing onboarding flow; a "convert this repo" flow can
  compose the two.
