# Tutti: design (desktop app, increment 1 - shell + board + live run)

Date: 2026-07-20
Status: implemented. The `tutti-app` Tauri + Svelte app ships the shell, the tracking
board, and a live continuous run (engine event/cancel hooks + `tutti-app-core` board
assembly). Increments 2 to 4 (multi-project switching, forge browsing, repo/group creation)
remain the rest of the desktop-app work.
Builds on: the full backend on `main` (engine + three forge adapters + unified status + CLI forge selection).

## The desktop app, and where increment 1 sits

The Tutti desktop app (slice 4) is a Tauri + Svelte application. It is large, so it is
built as an ordered sequence of separately-mergeable increments:

1. **Shell + board + live run (this spec).** The app shell, an embedded engine, the
   tracking board, and a live continuous run.
2. Multi-project switching (persisted list, add-by-directory).
3. Browse and add existing forge projects (forge listing + clone-on-add).
4. Create a new repo/project/group + bootstrap (the onboarding wizard).

Increment 1 is the foundation every later increment hangs on. It ships a usable app: open
a project, see its board, press Run, watch issues flow to done.

## Decisions (settled in brainstorming)

- **Embed `tutti-core` in the Tauri Rust backend.** Board reads call the `Forge` trait
  directly; runs call the engine in-process. No subprocess or stdout parsing. The backend
  reuses the CLI's `wire::build` to construct the `Box<dyn Forge>` from a project's config.
- **Frontend: Svelte + Vite** (TypeScript).
- **Control model: a continuous drain loop.** Run ships ready issues one after another
  until there is no ready work or the user pauses. **Pause is finish-then-stop**: the
  current issue completes cleanly, then the loop stops; Resume continues.
- **Live granularity: issue-lifecycle events.** The engine emits `DrainStarted`,
  `IssueClaimed`, `IssueShipped`, `IssueReleased`, `DrainComplete`. The board flips cards
  and updates counts live. Per-stage streaming (implement/review/gate/merge) is a later
  slice (the subsession view).
- **Shell layout:** a left **sidebar** (projects + app nav), a **main** pane with a
  `Board | Lanes` segmented toggle, and a **roadmap rail pinned right**. Selecting an issue
  opens a **slide-in detail drawer** (the board stays visible behind it). The left sidebar
  and the right rail are **resizable** (drag handle on the inner edge; widths persisted).
- **Board scope: all issues by default.** The board lists all of the repo's issues (via
  `Forge::list_issues`), bucketed by status label; the roadmap rail's milestones (plus an
  "All issues" entry) are an optional filter. (Earlier drafts scoped the board to a single
  milestone, which showed nothing for a repo with no milestones.)
- **Project source (increment 1 only):** a single active project loaded from a local
  directory containing a `tutti.toml`. The sidebar renders it; add/switch/browse/create
  arrive in increments 2 to 4.

## App structure

A new top-level `tutti-app/` whose `src-tauri` crate is named `tutti-app`. It is kept out
of the Cargo workspace's default-members gate path (its Tauri/webview deps would slow the
core gate) but still builds/tests as its own package (`cargo test -p tutti-app`). Layout:

```
tutti-app/
├── src-tauri/            # Rust: Tauri commands, app state, engine driver
│   ├── src/
│   │   ├── main.rs       # Tauri builder, command registration
│   │   ├── commands.rs   # load_project, get_board, start_run, pause_run, get_issue
│   │   ├── state.rs      # AppState: active project + run handle + pause flag
│   │   ├── board.rs      # assemble the board model from Forge reads
│   │   └── driver.rs     # the continuous run task + event forwarding
│   └── Cargo.toml        # deps: tutti-core, the 3 forge crates, tutti-git,
│                         #       tutti-backend-claude, tauri, tokio, serde
└── src/                  # Svelte + Vite frontend
    ├── routes/ or App.svelte
    ├── lib/
    │   ├── ipc.ts        # typed wrappers over Tauri invoke + event listeners
    │   ├── stores.ts     # board store, run-status store
    │   └── components/   # Sidebar, TopBar, Board, Lanes, RoadmapRail, IssueDrawer
    └── ...
```

The command layer stays thin: it validates input, calls into `board`/`driver`, and
returns serde types. The board assembly and the run driver hold the logic and are
unit-testable against `FakeForge`.

## The one `tutti-core` change: engine events + cooperative cancel

The engine's `drain()` loop (`crates/tutti-core/src/engine.rs`) drains up to
`max_issues_per_run` issues per call. Increment 1 adds two optional, injected collaborators
that the drain loop consults **between issues** (never mid-issue):

- **An event sink.** A new `EngineEvent` enum in `tutti-core`:
  `DrainStarted`, `IssueClaimed { id, title }`, `IssueShipped { id }`,
  `IssueReleased { id }`, `DrainComplete { shipped }`. The sink is an
  `Option<tokio::sync::mpsc::UnboundedSender<EngineEvent>>` (or a small `EngineSink`
  trait); when absent, emission is a no-op.
- **A cancel flag.** `Option<Arc<AtomicBool>>` checked at the top of each drain iteration;
  when set, the loop stops after the issue in flight finishes (finish-then-stop).

These are threaded without breaking existing callers: `drain()` stays back-compatible (the
CLI and tests pass neither and behave exactly as today). The exact injection shape
(constructor arguments vs a builder method vs an `EngineHooks` struct) is a plan decision;
the contract is: events emitted at the five lifecycle points, cancel honored between
issues.

The app's driver owns both: Pause sets the flag; the sink's receiver is forwarded to the
frontend as Tauri events.

## Backend: commands, state, driver

**AppState** (behind a `Mutex`/`RwLock` in Tauri's managed state):
- the active project: `Config`, the resolved `Box<dyn Forge>`, `repo`, `repo_root`;
- run status: `Idle | Running | Pausing`, current issue, shipped count;
- the run task `JoinHandle` and the shared cancel flag.

**Commands** (Tauri `#[command]`, all async, returning `Result<T, String>`):
- `load_project(dir: String) -> ProjectSummary` - read `tutti.toml` (reusing `Config` +
  `[forge]`), build the forge, store as active, return name/forge/repo.
- `get_board() -> Board` - `roadmap()` + `list_milestones()` + `milestone_children()` for
  the selected milestone (default: earliest open), shaped into
  `Board { milestones: Vec<MilestoneRow>, columns: { ready, in_progress, done } }`.
- `get_issue(id) -> IssueDetail` - the issue's title, body, labels, milestone, derived
  status and branch name, and a forge web URL for "Open in ...".
- `start_run()` - spawn the continuous drain task if idle; set status Running.
- `pause_run()` - set the cancel flag; status Pausing until the task ends.

**Driver** (`driver.rs`): the continuous loop. While not cancelled, call the engine's
drain path; between issues the engine checks the cancel flag and emits lifecycle events.
When a drain returns 0 shipped (no ready work) or the flag is set, stop and set status
Idle. Events from the sink receiver are re-emitted to the webview via
`app_handle.emit("engine://progress", event)`.

## Frontend: views and stores

- **Sidebar**: the active project (forge-colored dot); `+ Add project` and the
  Orchestrator/Subsessions nav rendered but disabled ("soon"). Real add/switch is
  increment 2.
- **TopBar**: project name, `Board | Lanes` segmented toggle, Run/Pause, and a status line
  (idle/running/pausing, current issue, shipped count).
- **Board (A)**: Ready / In-progress / Done columns of issue cards. **Lanes (B)**:
  milestone swimlanes with status-colored chips. The toggle swaps only the main pane.
- **RoadmapRail**: milestones with progress bars, pinned right; clicking one scopes the
  board.
- **IssueDrawer**: slides in over the right on card click; shows `get_issue` detail;
  dismissable; board stays live behind it.
- **Stores**: a `board` store (seeded by `get_board`, mutated by `engine://progress`
  events so cards flip live, reconciled by a full `get_board` on `DrainComplete`), and a
  `runStatus` store.
- **ipc.ts**: typed `invoke` wrappers and a typed listener for `engine://progress` so the
  Rust `EngineEvent`/`Board` shapes and the TS types stay aligned (mirror the serde types).

## Testing

- **Rust (hermetic, required):** board assembly from `FakeForge` reads (columns grouped by
  status label, milestone rows with progress); the engine's event emission and cancel
  behavior against a scripted `FakeForge` run (assert the `EngineEvent` sequence for a
  multi-issue drain, and that setting the cancel flag stops after the current issue). The
  Tauri command layer is thin enough to test via the `board`/`driver` functions directly.
- **Frontend:** a store-reducer test (apply a sequence of `engine://progress` events,
  assert the board store ends in the right shape) and a component smoke render. No e2e.
- **Manual/dev:** `cargo tauri dev` against the live GitLab sandbox drives a real board +
  run; not part of the required gate.

## Scope boundaries (out of increment 1)

- Multi-project add/switch/persist (increment 2); forge browsing (3); repo/group creation
  and bootstrap (4).
- Orchestrator chat and the subsession per-stage stream (later slices).
- Creating/editing issues or milestones from the UI; model management.
- A packaged/signed app bundle (dev-run suffices; packaging is a follow-up).

## Open questions and risks

- **Workspace build cost.** `src-tauri` pulls in Tauri and a webview toolchain; keep it out
  of the default `cargo test --all` gate path (own package, own CI job) so the core gate
  stays fast. Confirm during the plan.
- **Run vs project switch.** In increment 1 there is one project, so one run; when
  multi-project lands (increment 2), decide whether a run is per-active-project and what
  switching does to a live run. Out of scope here, noted so the state model leaves room.
- **Event/board reconciliation.** Live card flips from events must not drift from forge
  truth; the `DrainComplete` full refresh is the reconciliation point.
