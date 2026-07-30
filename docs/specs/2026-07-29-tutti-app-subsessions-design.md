# Subsessions per-stage stream (design)

Issue: [#25](https://github.com/doyled-it/tutti/issues/25). Part of the slice-4/5
desktop-app surface set. The slice-5 Tauri UI was "orchestrator chat + subsession chat +
tracking board"; the board and the orchestrator chat (#15) shipped, this is the third.

## What it is

A live, read-only view of an in-flight engine run. As the drain loop drives one issue
through Implementer -> Reviewer -> (FixApplier) and the Planner runs at the end of a pass,
each role's `claude -p` turn streams into its own subsession: the assistant text, the
tool-use asides, and a one-line outcome when the turn finishes. You watch what each role is
doing, not just the board-level claimed/shipped transitions.

It observes the run. It never drives it: there is no compose box, and it emits no command
back to the engine. Persistence and cross-run history are out of scope; the view is the
current run, cleared when the next run starts.

## Why it is reachable

The plumbing is already in place and mostly just discarded today:

- `engine.rs::run_role` (the single choke point every role turn passes through) already
  creates an `AgentEvent` channel per turn and **throws the whole stream away**:
  `tokio::spawn(async move { while rx.recv().await.is_some() {} })`. That discard is exactly
  the subsession stream. `run_role` already receives `issue: &Issue` and `role: Role`, so
  tagging each event is free.
- `EngineHooks` (`events.rs`) already carries the `EngineEvent` sink threaded through the
  whole drain path and defaults to inert. A parallel subsession sink slots in the same way.
- `tutti-app/src-tauri/src/orchestrator.rs` is the exact `AgentEvent` -> Tauri-event
  template, and `src/lib/orchestrator.ts`'s pure `appendDelta` / `appendTool` /
  `dropTrailingEmptyAssistant` are role-agnostic and reused directly.
- `Role` derives `Copy + Hash + Serialize` (snake_case), so `(issue_id, role)` is a clean
  subsession key on the wire and on the frontend.

## Backend (`tutti-core`)

### `SubsessionEvent` (new, in `events.rs`)

Serde-tagged (`kind`, snake_case) so the webview forwards it verbatim, mirroring
`EngineEvent`. Every variant is keyed by `issue: u64` + `role: Role`.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubsessionEvent {
    /// A role turn began. `title` is the issue title (carried so the pane needs no
    /// lookup); for the Planner the synthetic planner issue's title.
    Started { issue: u64, role: Role, title: String },
    /// A chunk of assistant text.
    Delta { issue: u64, role: Role, text: String },
    /// A tool-use aside (the tool name).
    Tool { issue: u64, role: Role, name: String },
    /// The turn finished. `summary` is a role-aware one-liner; `ok` drives the status dot.
    Completed { issue: u64, role: Role, summary: String, ok: bool },
}
```

`role` requires `events.rs` to import `crate::message::Role`.

The `Completed.summary` is composed in `run_role` from the `AgentOutcome` (or error) already
in hand, so the pane stays dumb:

- Implementer / FixApplier: `ok = status == ReadyToShip`; `"ready to ship"` or
  `"blocked: {reason}"`.
- Reviewer: `Approve -> ("approved", true)`; `RequestChanges -> ("request changes (N findings)", false)`.
- Planner: `("plan: {action:?}", true)`.
- A `run_role` that returns `Err` (backend/spawn failure): `("error: {e}", false)`.

A small pure helper `subsession_summary(role, &Result<AgentOutcome>) -> (String, bool)` holds
this mapping so it is unit-testable in isolation.

### `EngineHooks` gains a second sink

```rust
pub struct EngineHooks {
    pub sink: Option<UnboundedSender<EngineEvent>>,
    pub cancel: Option<Arc<AtomicBool>>,
    /// Where per-role turn streams go. `None` = no emission (CLI, tests): unchanged behavior.
    pub subsession: Option<UnboundedSender<SubsessionEvent>>,
}
```

`Default` stays derivable (`None`), and `Clone` stays (the sender is `Clone`). An
`emit_subsession(&self, ev)` helper mirrors `emit`. All existing constructions
(`EngineHooks::default()`, the two `driver.rs` literals, the test literals) get the new field
defaulted to `None` except the app driver.

### `run_role` forwards instead of discarding

`run_role` gains `hooks: &EngineHooks`. It:

1. emits `Started { issue.id.0, role, issue.title }`;
2. replaces the discard task with a forwarder that clones `hooks.subsession` (cheap
   `Option<UnboundedSender>`), captures the `Copy` `issue_id` + `role`, and maps each
   `AgentEvent` to a `SubsessionEvent` (`Line -> Delta`, `ToolUse -> Tool`, `Done` dropped);
3. after `backend.run(...)` returns, emits `Completed` with `subsession_summary(role, &out)`;
4. returns `out` unchanged.

Because the sink is optional and cloned into a `'static` task, and `issue_id`/`role` are
`Copy`, there is no new borrow crossing the spawn. When `subsession` is `None` the forwarder
still drains the channel (so the backend never blocks) but sends nothing: behavior identical
to today.

Call-site threading (mechanical, no control-flow change):

- `run_stages`'s three `run_role` calls already hold `hooks`.
- `plan()` gains `hooks: &EngineHooks`; `drain_with` passes its `hooks` to `self.plan(hooks)`.
  This is what covers the Planner subsession.
- `run_one()` / `drain()` (back-compat entry points) delegate with `EngineHooks::default()`,
  whose `subsession` is `None`: no-op, as before.

### CLI unchanged

Subsessions is a GUI observation surface. `tutti-cli` keeps its default (no-op) hooks; there
is no dual-driver wiring here (unlike the codegraph context work, which had to touch both
drivers). The engine emits nothing extra unless an app driver installs a sink.

## Tauri wiring (`tutti-app/src-tauri`)

A new `subsession.rs` module (sibling to `orchestrator.rs`), plus a small `driver.rs` change:

- `driver.rs::start` builds a second `unbounded_channel::<SubsessionEvent>()`, stores the
  sender in `hooks.subsession`, and spawns a forwarder:
  `while let Some(ev) = rx.recv().await { app.emit("subsession://event", &ev); }`. The
  receiver end is threaded through `run_loop` alongside the existing `EngineEvent` `tx`.
- No new Tauri **command** is needed: the pane is populated entirely by the pushed event
  stream and reset by the existing run-start path. (`subsession.rs` holds the forwarder
  helper and the event-name constant so `driver.rs` stays lean; if it turns out to be a
  one-liner it may fold into `driver.rs` directly.)

## Frontend

### `src/lib/subsessions.ts` (new, pure, vitest-covered)

Follows the `board.ts` / `browse.ts` convention: pure logic here, wiring in the pane. Reuses
`orchestrator.ts`'s `ChatMessage`, `appendDelta`, `appendTool`, `dropTrailingEmptyAssistant`
(reuse, not fork).

```ts
export type SubStatus = "running" | "done" | "error";

export interface Subsession {
  key: string;               // `${issue}:${role}`
  issue: number;
  role: Role;                // "implementer" | "reviewer" | "fix_applier" | "planner"
  title: string;
  status: SubStatus;
  messages: ChatMessage[];   // orchestrator.ts ChatMessage
  summary?: string;
}

export interface SubsessionState {
  list: Subsession[];        // insertion order (stable left-rail order)
  selected: string | null;   // key
}

export function emptySubsessions(): SubsessionState;
export function applySubsession(state: SubsessionState, ev: SubsessionEvent): SubsessionState;
```

`applySubsession` behavior:

- `started`: upsert the `(issue, role)` subsession, resetting its transcript to an open
  assistant bubble (`startAssistant([])`) and status `running`. Set `selected` to this key so
  the view follows the live edge (the user can click back to a finished one; a later `started`
  will advance again). A re-attempt of a released issue reuses the key and resets cleanly.
- `delta`: route the text to the matching key via `appendDelta`. Defensive no-op if the key
  is unknown (an out-of-order delta).
- `tool`: route via `appendTool`.
- `completed`: set `status` (`ok ? "done" : "error"`), store `summary`, and
  `dropTrailingEmptyAssistant` so the live view matches a tool-final turn.

`Role` on the frontend is `"implementer" | "reviewer" | "fix_applier" | "planner"` (snake_case
matches the serde wire), added to `ipc.ts`.

### `stores.ts`

- `export const subsessions = writable<SubsessionState>(emptySubsessions())`.
- `section` type extended to `"board" | "orchestrator" | "subsessions"`.
- `clearSubsessions()` resets the store (called on run start).

### `ipc.ts`

- `Role` type.
- `SubsessionEvent` discriminated union mirroring the Rust serde tag.
- `onSubsession(cb)` listening on `subsession://event`.

### `+page.svelte`

- In `onMount`, add an `onSubsession` listener that folds each event into the `subsessions`
  store (`subsessions.update(s => applySubsession(s, ev))`), mounted globally like
  `onProgress` so it populates even while the Board is on screen.
- `run()` calls `clearSubsessions()` alongside the existing `runStatus` reset. This is the
  "new run start" signal; it lives here already.
- Render `SubsessionsPane` in the work area when `section === "subsessions"`.

### `SubsessionsPane.svelte` (new)

Master-detail:

- Left: the subsession list. Each row: `#{issue} {role label}` (the Planner shows just
  "Planner", no `#0`), with a status dot (running = accent/pulsing, done = teal/check,
  error = coral). Clicking selects.
- Right: the selected subsession's transcript, rendered exactly like `OrchestratorPane`
  (assistant bubbles + `ran {tool}` asides, `white-space: pre-wrap`), a header of
  `{role label} · #{issue} {title}`, and the outcome footer (`summary` with the status
  glyph). No compose box.
- Empty state when the list is empty ("Start a run to watch its stages.").

### `Sidebar.svelte`

Replace the `<div class="nav-item soon">Subsessions (soon)</div>` with a real nav button
wired to `onSection?.("subsessions")`; extend the `section` prop union and the `onSection`
signature. (The nav no longer has any `soon` placeholder.)

## Testing

- **Rust, hermetic** (sibling to `drain_emits_lifecycle_events`): drive a two-issue drain with
  a `subsession` sink and assert, per role, a `Started`, at least one `Delta` (FakeBackend
  already emits `AgentEvent::Line("fake {Role}")`), and a `Completed`, with correct
  `(issue, role)` keys. Assert an `Approve` review yields `Completed{ok:true}` and a blocked
  implementer path yields `Completed{ok:false}` with the reason in the summary. Unit-test
  `subsession_summary` directly across all four roles + the error case.
- **Rust, back-compat**: a drain with `EngineHooks::default()` (no subsession sink) still ships
  and emits nothing extra (the discard-equivalent path).
- **Frontend, vitest** (`subsessions.test.ts`): `applySubsession` over
  started/delta/tool/completed/clear; live-edge selection advancing on each `started` and the
  user's manual selection surviving until the next `started`; Planner keying (`0:planner`,
  label "Planner"); an unknown-key delta is a no-op.
- **Manual / live** (noted, not CI, matching Tutti's spike-then-trust rhythm): run the app
  against a real project, start a drain, and watch each role's pane stream and its outcome
  footer land. This is the acceptance check the hermetic tests cannot exercise.

## Scope / non-goals

- One PR, `feat/app-subsessions` off `main`, "Part of #25".
- Live-only, read-only. No compose, no persistence, no cross-run history.
- No full artifact rendering (no findings list, no diff, no handoff body): the streamed text
  already shows the agent producing them, and `Completed.summary` is the structured takeaway.
- No CLI change; no new Tauri command.
