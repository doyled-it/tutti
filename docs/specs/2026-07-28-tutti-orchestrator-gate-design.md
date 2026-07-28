# Set the gate from the orchestrator conversation

Design for issue #15. Builds the full orchestrator chat surface and uses it to set a
project's verification gate, replacing the hand-edit-`tutti.toml` gap the wizard leaves
behind (every wizard-created project ships on the no-op gate `["true"]`).

## Why the gate is not a setup question

The wizard deliberately seeds `gate = ["true"]` (see
`2026-07-22-tutti-app-init-wizard-design.md`, "gate_commands"). What must pass before
Tutti ships an issue's work is not a setup fact. It falls out of the conversation about
what the project is and how it gets verified, at the moment the user knows the most, not
the least. So the gate is set from the orchestrator chat, and the chat is where this
increment invests.

## Scope and packaging

Two independently mergeable PRs (Tutti PRs go straight to main, so each must build,
pass CI, and demo on its own):

- **PR A. Orchestrator chat surface.** A real `Orchestrator` nav section driving
  `claude -p` as a resumable, persisted, streaming multi-turn session in the repo
  checkout, grounded by the same codegraph MCP context every other agent gets. Demoable
  alone: you can talk to a project-aware agent about your repo.
- **PR B. Gate proposal, write-back, and the no-op indicator.** The agent proposes a
  gate as a structured artifact, the app shows a confirm card, and on confirm the app
  does a surgical `toml_edit` write-back that preserves every other key. A board-level
  indicator flags a project still on the no-op gate. Depends on PR A.

PR B stacks on PR A: it branches off main after PR A merges. The design document lands
with PR A.

Non-goals: the subsession per-stage stream (a later app increment, though PR A's event
plumbing is built to seed it); any change to the engine's ship path or the existing
handoff/plan/review artifact readers; a tutti-served MCP tool surface.

---

# PR A. The orchestrator chat surface

## A1. Session model: process-per-turn, resumable

The chat drives `claude -p` as one process per user turn, reusing the proven one-shot
spawn model from `ClaudeBackend` rather than a long-lived stdin/stdout stream. Each turn:

```
claude -p "<user message>" \
  --output-format stream-json --verbose \
  --model <config.model> \
  --mcp-config <codegraph config>   # non-strict, same as the engine path
  [--resume <session_id>]           # turns 2..N
```

run with `cwd = repo_root`. Running in the repo checkout is what lets the agent read
real project files; running there consistently also keeps claude's own session store
(which claude keys by working directory) stable across turns so `--resume` finds the
session.

**Session id: captured, not generated.** Turn 1 runs with no session flag; the backend
reads the `session_id` claude reports in its `system` init line and `result` line and
stores it. Turns 2..N pass `--resume <session_id>`. Capturing avoids gambling on
`--session-id` acceptance semantics. The exact resume/session-id flag spelling gets a
**live spike at build time** against the installed `claude`, following Tutti's rhythm of
spiking the real CLI before wiring an adapter (the backend, forge, and browse increments
all did this). If the spike shows `--session-id <uuid>` is reliable with `-p`, generating
our own id is an acceptable equivalent; the design does not depend on which.

**Code shape.** This lands as a new `session` module in `tutti-backend-claude`, not
inside `ClaudeBackend::run`. `run` is built around `AgentTask -> AgentOutcome` with a
ship artifact; a chat turn has an entirely different outcome (streamed assistant text, a
captured session id, an optional proposal, no ship). The reusable assets are shared:
`stream.rs` (already parses assistant text blocks, `tool_use`, and the `result` event)
and the spawn + concurrent-stderr-drain plumbing. `stream.rs` gains the ability to
surface the `session_id` from the `system`/`result` lines (additive; existing parsing
unchanged).

The codegraph wiring reuses the existing seam: `tutti_core::mcp::write_mcp_config` writes
the config to an OS temp dir (never the worktree), and the session passes `--mcp-config`
non-strict exactly as `ClaudeBackend::run` does. The chat is gated by
`config.codegraph_enabled()` and the binary's presence the same way, so a machine without
codegraph is a clean no-op.

## A2. Transcript persistence

A pure `OrchestratorStore` in `tutti-app-core`, mirroring `ProjectStore`: parse-from-json
tolerant of an absent file, serialize-to-json, unit-tested. Serialized to
`app_data_dir()/orchestrator/<key>.json`, one file per project, **keyed by the project
`dir`** (the repo-root path string, the same identity `ProjectStore` keys on). The key is
sanitized into a filename-safe stable string.

```jsonc
{
  "session_id": "…" | null,          // claude session to --resume; null before turn 1
  "messages": [
    { "role": "user",      "text": "…" },
    { "role": "assistant", "text": "…", "kind": "text" },
    { "role": "assistant", "text": "ran cargo metadata", "kind": "tool" },
    { "role": "assistant", "text": "…", "kind": "proposal", "proposal": { … } }  // PR B
  ]
}
```

`kind` distinguishes plain assistant prose from a tool-use notice and (in PR B) a gate
proposal, so the pane can render each differently on reload. On project load the backend
reads this file; the pane rehydrates the transcript and the stored `session_id` lets the
next turn `--resume` where the user left off. The store lives in `app_data_dir`, never in
the repo, so it is immune to the `commit_all` `git add -A` sweep and survives across
re-clones.

Writes are whole-file after each turn completes (the transcript is small and turns are
sequential). A user message is appended and persisted before the turn runs, so a crash
mid-turn does not lose the prompt.

## A3. Streaming to the webview

New Tauri events, siblings of `engine://progress`, emitted from the turn driver:

- `orchestrator://delta` — an assistant text chunk (append to the in-flight bubble).
- `orchestrator://tool` — a tool-use notice (the tool name), rendered as a muted aside.
- `orchestrator://done` — the turn finished. The frontend uses it as the turn-complete
  signal (clears the "thinking" state and drops any trailing empty bubble), the same way
  the board uses `engine://run-ended` rather than a per-pass event. The streamed deltas are
  the source of truth for the on-screen transcript, and the authoritative reply is persisted
  backend-side, so the payload text is not consumed by the pane (a dropped delta self-heals
  on the next reload via `get_transcript`). The captured `session_id` is persisted with the
  transcript backend-side and is not sent to the UI, which never needs it. (PR B attaches any
  parsed proposal to this event.)
- `orchestrator://error` — the turn failed (spawn failure, non-zero exit, rate limit),
  carrying a short reason. The pane shows it inline and re-enables the composer.

The turn driver (an app-side module beside `driver.rs`) spawns the session turn, forwards
each parsed stream event onto the matching Tauri event, persists the completed turn, and
returns. Mapping the `stream.rs` events (`Line` -> delta, `ToolUse` -> tool, `Done` /
`result` -> done) reuses the parser wholesale. This event plumbing deliberately mirrors
the engine's so the future subsession per-stage stream can adopt the same shape.

A single-flight guard: only one orchestrator turn runs at a time. The composer is disabled
while a turn is in flight, and the backend refuses a second concurrent
`send_orchestrator_message` (an `orchestrator_busy` flag) so two turns cannot interleave the
transcript read-modify-write even if the frontend guard is bypassed. While a turn is in
flight the sidebar also blocks project switch/add/remove (the same posture as an active
engine run), so a project cannot be swapped out from under a running turn. This is
independent of the engine run state, so you can brainstorm the gate while nothing is draining.

## A4. Navigation and the pane

The sidebar nav gains real section-switching. `Board` and `Orchestrator` become active,
selectable items driving a `section` state; the center pane swaps between the existing
board/lanes view and the new `OrchestratorPane`. `Subsessions` stays a `soon` placeholder.

`OrchestratorPane.svelte`: a scrolling transcript (user bubbles, assistant bubbles, tool
asides) plus a compose box that becomes a disabled/"thinking" state while a turn is in
flight. Assistant bubbles render as **plain text** (`white-space: pre-wrap`), not markdown:
the reply streams in delta by delta and re-rendering partial markdown on every chunk is
janky, and plain text is also XSS-safe by construction. Rendering the final reply as
sanitized markdown (the `marked` + `DOMPurify` path the issue drawer uses) is a reasonable
later enhancement, deferred here. Pure view helpers (event-to-transcript reduction, message
classification, trailing-empty-bubble cleanup) live in a vitest-covered
`src/lib/orchestrator.ts`, following the `board.ts` / `browse.ts` / `create.ts` helper
convention, so the reducer is tested without the webview.

## A5. Agent permission posture

The chat drives `claude -p --dangerously-skip-permissions`, the same posture as the
autonomous backend. This is partly forced: headless `-p` cannot answer interactive
permission prompts, so a turn with prompts enabled would hang. The consequence to be
explicit about is that the chat agent is **not read-only**: it can edit files and run
commands in the repo checkout, so "talk to the agent about the project" can, if the user
asks, become "the agent changes the project." This is acceptable for PR A (a single-user
local tool, the same trust already extended to the engine), but it is a deliberate decision,
not an oversight. PR B, where the agent's job is to *propose* a gate rather than *apply*
one, is the natural place to constrain the chat to a read-oriented tool set (`claude`'s
`--allowedTools` / a restrictive permission mode), pending a live spike of the exact flags.

---

# PR B. Gate proposal, write-back, and the no-op indicator

## B1. The structured proposal frame (handoff-style)

The agent proposes a gate by writing an artifact file, exactly like the existing
`handoff.json` / `plan.json` / review artifacts. The turn prompt passes an **absolute
temp path outside the repo** (per the codegraph gotcha: anything under the worktree is
swept into a PR by `commit_all`'s `git add -A`) and instructs the agent: when you and the
user have agreed on what verifies this project, write your proposal to `<path>` as

```json
{ "commands": ["cargo test"], "working_dir": "", "rationale": "why these commands" }
```

After each turn the backend deletes-then-checks that path (the same stale-artifact
discipline `ClaudeBackend::run` uses on `out_path`), parses a present file, and attaches
the proposal to that turn's `orchestrator://done`. The agent never edits `tutti.toml`;
applying the proposal is the app's job.

This is chosen over an inline fenced ```gate-proposal``` block (fragile prose parsing) and
over a tutti-served MCP tool (real new stdio-server infrastructure for a single call). The
file-artifact pattern is the one the codebase already trusts across every role.

The pane renders a present proposal as an **inline confirm card** in the transcript:
"Set gate to `cargo test`? [Apply] [Dismiss]", showing the commands and the rationale.
Dismiss just drops the card (the agent can propose again). Apply calls the write-back.

## B2. The `toml_edit` write-back seam

Add `toml_edit` to `tutti-app-core`. A pure, unit-tested function:

```rust
pub fn set_gate_commands(existing_toml: &str, commands: &[String]) -> Result<String, ...>
```

Surgically sets `[gate].commands` and preserves everything else: `merge_mode`, `roles`,
`[gate].working_dir`, key order, hand edits, and comments. It handles all three shapes:

- `[gate]` table with a `commands` array -> replace the array in place.
- `[gate]` table without `commands` -> insert the key.
- no `[gate]` table -> create it.

Round-trip tests assert that a file carrying comments, a custom `merge_mode`, and an
existing `[gate].working_dir` survives a `set_gate_commands` with only `commands` changed,
and that the result still parses and validates via `Config::load`.

`render_tutti_toml` (generation of a brand-new file) is untouched; it hand-writes TOML for
a file that does not exist yet, a different job from editing one that does.

The `apply_gate` Tauri command is a thin wrapper: read `tutti.toml`, call
`set_gate_commands`, write it back atomically, then **reload `Config` and replace
`Project.config` in managed state** so the indicator reflects the new gate immediately,
and return the new gate status. It is run-guarded (blocked while a run is active) like the
other mutating commands, since it changes what a drain would verify.

## B3. The board no-op indicator

A pure helper in `tutti-app-core`:

```rust
pub fn gate_is_noop(commands: &[String]) -> bool   // commands == ["true"]
```

surfaced through a small `get_gate_status` command returning `{ commands, is_noop }`,
called on project load and after `apply_gate`. When `is_noop`, the app shows a persistent
**TopBar badge**: "No verification gate — nothing is checked before ship", clickable to
switch to the Orchestrator section. TopBar rather than a board column because the gate is
a project-level fact that should be visible from both the board and the orchestrator view,
and because it is computable the moment a project loads (no forge round-trip).

## Scope boundary

PR B adds no engine behavior. It reads config that already loads, writes one config key,
and reloads. The proposal reader follows the existing artifact-reader shape and touches
none of the handoff/plan/review paths.

---

## Testing

Following the repo's hermetic-first, live-spike-second convention:

- `tutti-app-core`: `OrchestratorStore` json round-trip and absent-file tolerance;
  `set_gate_commands` preservation round-trips (comments, `merge_mode`, `working_dir`,
  the three table shapes) and post-edit `Config::load`; `gate_is_noop` truth table.
- `tutti-backend-claude`: `stream.rs` session-id extraction from captured `system`/
  `result` fixture lines; the proposal reader (present / absent / malformed) mirroring the
  handoff-reader tests.
- Frontend: `src/lib/orchestrator.ts` reducer (delta accumulation, tool asides, proposal
  attachment, done finalization) under vitest.
- Live spike (opt-in `live`, `#[ignore]`, human-run): a real `claude -p` turn plus a
  `--resume` second turn in a scratch checkout, confirming the session-id capture and
  resume flags, and a real proposal-artifact round-trip. Manual GUI smoke of the pane,
  the confirm card, the write-back preserving a hand-edited `tutti.toml`, and the badge.

## Gotchas to carry forward

- The proposal artifact and any transient file the agent is told to write MUST live
  outside the worktree (absolute temp path in the prompt), or `commit_all`'s `git add -A`
  sweeps it into the user's PR. Same rule the codegraph `--mcp-config` file follows.
- claude keys its resumable session store by working directory; the chat must always run
  in the same `repo_root`, or `--resume` will not find the session.
- The transcript store is app-owned (`app_data_dir`), never in the repo, so it neither
  pollutes git status nor gets committed and survives a re-clone.
- `toml_edit`, not `toml`, for the write-back: `toml` (used by `Config::load`) does not
  preserve formatting or comments; a `toml`-based rewrite would erase hand edits.
