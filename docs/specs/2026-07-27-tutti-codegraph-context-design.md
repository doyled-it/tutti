# Codegraph context for every agent invocation

Part of #21. Precedes #15 (the orchestrator chat reuses this seam).

## Problem

Every LLM agent function Tutti drives (planner, implementer, reviewer, fixer today;
the orchestrator chat next) reconstructs a repo's structure by crawling files. That is
slow and token-heavy. [codegraph](https://github.com/colbymchenry/codegraph) pre-indexes
a repo into a symbol / call-graph SQLite database and exposes a `codegraph_explore` tool
over a stdio MCP server (`codegraph serve --mcp`), so an agent answers structural
questions from the index in one call. We want Tutti to give this context to every agent
function, for every repo it operates on, across all backends.

## Goals

- Wire the codegraph MCP server into every `claude -p` invocation Tutti makes.
- Ensure the target repo is indexed so `codegraph_explore` has data to return.
- Build it as a backend-agnostic seam so Codex/OpenCode inherit it when they land.
- Never regress the plain path: a machine without `codegraph` behaves exactly as today.

## Non-goals

- The orchestrator chat and gate-setting (#15). It is the next increment and reuses this
  seam; nothing chat-specific is built here.
- Persisting or caching anything Tutti-side. codegraph owns its own `.codegraph/` DB and
  incremental sync.
- Per-role tuning of codegraph, multiple context providers, or a provider trait. There is
  one provider (codegraph); a trait is deferred until there is a second (YAGNI).
- Mutating the user's global `~/.claude.json` or writing marker fences into tracked repo
  files (`CLAUDE.md`/`AGENTS.md`). Wiring is per-invocation and Tutti-owned.

## Decisions (from brainstorming)

- **Managed `--mcp-config`, not `codegraph install`.** Tutti generates an explicit MCP
  config file and passes it per `claude -p` call. Hermetic, reversible, per-backend, and
  the injection point is the backend-agnostic seam. No global side effects.
- **Seam lives on the backend invocation, keyed off data in `tutti-core`.** A neutral
  `McpServer` value type, produced by a codegraph provider, consumed by whichever backend
  runs. Claude → `--mcp-config`; a future backend that cannot do MCP ignores the specs.
- **Best-effort, never fatal.** Missing binary, index failure, or a config write error
  logs and proceeds with a plain (un-wired) agent run.

## Architecture

### 1. `tutti-core::mcp` (new module)

```rust
/// A stdio MCP server Tutti can hand to a backend. Backend- and forge-neutral data.
pub struct McpServer {
    pub name: String,      // "codegraph"
    pub command: String,   // "codegraph"
    pub args: Vec<String>, // ["serve", "--mcp"]
}

/// Serialize a set of servers to the Claude Code `--mcp-config` shape and write it to
/// `dir/mcp-config.json`, returning the path. Shared by the engine backend path and the
/// future orchestrator-chat path so both write an identical file.
pub fn write_mcp_config(servers: &[McpServer], dir: &Path) -> Result<PathBuf>;
```

The file shape matches what `claude -p --mcp-config` expects:

```json
{ "mcpServers": { "codegraph": { "type": "stdio", "command": "codegraph", "args": ["serve", "--mcp"] } } }
```

### 2. `tutti-core::context` — the codegraph provider

```rust
pub struct CodeGraph { /* resolved binary path */ }

impl CodeGraph {
    /// `None` when the `codegraph` binary is not on PATH (probed via `codegraph --version`).
    pub fn detect() -> Option<CodeGraph>;
    /// Ensure `dir` has a codegraph index (`codegraph init` when `.codegraph/` is absent;
    /// codegraph's own file watcher keeps it fresh afterward). Best-effort.
    pub async fn ensure_indexed(&self, dir: &Path) -> Result<()>;
    /// The MCP server spec for `codegraph serve --mcp`.
    pub fn mcp_server(&self) -> McpServer;
}
```

`detect()` returning `None` is the graceful-absence path: the engine wires nothing and
runs plainly.

### 3. Backend integration (`AgentBackend` / `ClaudeBackend`)

- Add `mcp_servers: Vec<McpServer>` to `AgentTask` (default empty → current behavior;
  back-compat for every existing construction site and test).
- `ClaudeBackend::run`: when `task.mcp_servers` is non-empty, call `write_mcp_config`
  into the worktree and append `--mcp-config <path>` to the `claude` command. Non-strict
  (omit `--strict-mcp-config`) so any user-global servers still load. When empty, the
  command is byte-for-byte what it is today.
- `prompt.rs`: append a one-line nudge when servers are present, e.g. "A `codegraph_explore`
  MCP tool is available; prefer it over reading files to map structure, callers, and impact."
  This replaces the fenced instructions `codegraph install` would have written.

### 4. Engine & lifecycle wiring (`engine.rs`)

- `Engine` gains an optional context provider (e.g. `Option<&dyn ...>` or `Option<CodeGraph>`;
  concrete `CodeGraph` is fine since it is the only provider — revisit if a trait is needed).
- Before each backend run (both `run_one` and the planner's `run_role`): call
  `ensure_indexed(workdir)` and set `task.mcp_servers = vec![provider.mcp_server()]`.
- Default provider `None` → `mcp_servers` stays empty → today's behavior, unchanged.
- Indexing errors are logged and swallowed; the run proceeds un-wired.

### 5. Wiring in the drivers (`tutti-app` and `tutti-cli`)

- Build the `CodeGraph` provider once (via `detect()`), gated by `config.codegraph_enabled()`,
  and hand it to the engine via `with_context`. When `None` or disabled, pass no provider.
- Both production drivers wire it identically: the Tauri app run driver
  (`tutti-app/src-tauri/src/driver.rs`) and the `tutti` CLI (`tutti-cli/src/main.rs`, which is
  how the autonomous engine runs). Wiring only one would leave the other production path
  without codegraph context, so both get the same `detect + gate + with_context` block.

## The worktree-indexing question — RESOLVED

Engine agents run in **transient git worktrees**. `codegraph serve --mcp` is spawned by
`claude`, which runs with the worktree as cwd. Probed against codegraph 0.9.3:

- `codegraph serve --mcp -p <path>` accepts an explicit **`-p, --path`** ("optional for MCP
  mode, uses `rootUri` from client" otherwise). Claude sends the worktree as `rootUri`, so
  **without** `-p` serve would index/serve the worktree. **With** `-p <main-working-dir>` it
  serves a fixed index regardless of the agent's cwd.
- `codegraph init -i <path>` initializes and runs the initial index in one step.

**Decision (option a):** index the **main working dir once** (`codegraph init -i <dir>`) and
put `-p <main-working-dir>` in the MCP server args so every agent — worktree-bound engine
roles, the planner (runs in the workdir), and the future chat — reads that single index.
The main index reflects trunk, not an agent's in-flight worktree edits; that is acceptable,
since codegraph is for understanding existing structure (callers, impact, entry points),
not the agent's own fresh diff. codegraph's watcher keeps the main index fresh.

Consequence for the seam: `McpServer` for codegraph is `{ command: "codegraph", args:
["serve", "--mcp", "-p", <main-working-dir>] }`, so `CodeGraph::mcp_server` takes the
project path. `ensure_indexed(dir)` shells `codegraph init -i <dir>` when `dir/.codegraph`
is absent.

## Config

Optional `[codegraph]` section in `tutti.toml`:

```toml
[codegraph]
enabled = true   # default true when the codegraph binary is present; false = full no-op
```

Absent section → default (on when the binary is detected). Absent binary → no-op
regardless. No other knobs in this increment.

## Error handling

- `codegraph` not on PATH → `detect()` is `None`; no wiring, no flag, plain run.
- `ensure_indexed` fails (init error, permission, timeout) → log once, proceed un-wired.
- `write_mcp_config` fails → log, drop the `--mcp-config` flag, proceed un-wired.
- A failing/malformed MCP server at agent runtime is Claude's concern, not fatal to Tutti.

No path here can turn "codegraph had a problem" into "the agent did not run."

## Testing

**Hermetic (default tier):**
- `write_mcp_config` produces the exact expected JSON and round-trips its path.
- `ClaudeBackend` adds `--mcp-config` iff `mcp_servers` is non-empty, and omits it
  otherwise (assert on the built arg list, mirroring `verbose_is_in_default_args`).
- The prompt nudge appears iff servers are present.
- Engine wiring: a fake provider populates `mcp_servers` and triggers `ensure_indexed`;
  a `None` provider leaves the task's `mcp_servers` empty (behavior-preserving).
- Graceful absence: no binary → no flag, run proceeds.

**Live tier (opt-in `live` feature, `#[ignore]`):**
- Against a real repo with `codegraph` installed: `ensure_indexed` creates `.codegraph/`,
  and a real `claude -p --mcp-config` run can call `codegraph_explore`. Skips cleanly when
  the binary is not on the box.

## Gotchas (confirmed against codegraph 0.9.3 / installed `claude`)

- **codegraph serve cwd resolution.** `serve --mcp` defaults to the MCP client's `rootUri`
  (the agent's worktree). Must pass `-p <main-working-dir>` to serve a fixed index instead
  of one tied to the transient worktree. See the resolved section above.
- **`claude` MCP flags.** `--mcp-config <configs...>` loads servers from JSON files;
  `--strict-mcp-config` restricts to only those files. Use `--mcp-config` **without**
  `--strict-mcp-config` so a user's own global MCP servers still load alongside codegraph.
- **codegraph CLI.** `codegraph init -i <path>` = init + initial index (the `-i` flag is
  required for the index). `codegraph --version` is the presence probe.
- **The `--mcp-config` file must NOT live in the agent worktree.** The engine ships an
  agent's work via `Workspace::commit_all`, which runs `git add -A` inside the worktree
  (there is no `.tutti` gitignore), so any file written to the worktree would be swept into
  the user's feature branch and PR. `ClaudeBackend` therefore writes the config to an OS
  temp dir (`std::env::temp_dir()/tutti-mcp-<pid>/mcp-config.json`) and passes that absolute
  path to `--mcp-config`. The file location does not affect behavior: codegraph's `-p`
  already points at the absolute main checkout.

## Rollout

One mergeable PR referencing `Part of #21`, straight to `main` (tutti has no CLAUDE.md gate
and no `version:*` labels). Assigned to `doyled-it`, labeled `enhancement`.
