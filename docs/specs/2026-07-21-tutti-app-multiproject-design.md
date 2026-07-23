# Tutti: design (desktop app, increment 2 - multi-project switching)

Date: 2026-07-21
Status: implemented. The sidebar holds a persisted project list (add by folder, switch, remove); the list and last-active board restore across launches; add/switch/remove are blocked while a run is active.
Builds on: desktop app increment 1 (shell + board + live run), on `main` (#13).

## What this increment is

Increment 1 loads one project (a local folder with a `tutti.toml`, repo auto-detected from
the git remote). Increment 2 makes the sidebar a real, persisted list of projects: add a
project by picking a folder, switch the active project by clicking it, remove one, and have
the list survive across launches.

## Decisions (settled in brainstorming)

- **Persist a project list as a plain JSON file** in the app data dir (dependency-free, no
  store plugin).
- **One active project, one run at a time.** Switching projects is free while idle but
  **blocked while a run is active** (the user pauses first). Concurrent per-project runs are
  out of scope for this increment.
- **Add = pick a folder** (repo auto-detected via the increment-1 `repo_from_remote` path);
  the entry is validated (its `tutti.toml` loads and its forge builds) before it is persisted.

## Persistence

A `projects.json` in `app_handle.path().app_data_dir()`:

```json
{
  "projects": [
    { "dir": "/Users/me/projects/app", "repo": "doyled-it/app", "name": "app", "forge": "gitlab" }
  ],
  "active": "/Users/me/projects/app"
}
```

- `dir` is the identity (a folder is a project). `repo` is the resolved slug, `name` the
  folder name, `forge` the kind (for the sidebar's colored dot).
- Read on startup; rewritten on every add/remove/switch. Missing or unparseable file =
  empty list (first run).

## Backend (Tauri): state + commands

`AppState` keeps the persisted `Vec<ProjectEntry>` (+ the active dir) alongside the one
built active `Project` and the run status.

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectEntry { pub dir: String, pub repo: String, pub name: String, pub forge: String }
```

Commands (all async, `Result<T, String>`), each taking `app: tauri::AppHandle` where they
touch the store:

- `list_projects() -> { projects: Vec<ProjectEntry>, active: Option<String> }` - read the
  store. Called on launch.
- `add_project(dir) -> ProjectEntry` - resolve repo (auto-detect, else error), load+validate
  `Config`, build the forge, determine name/forge kind, upsert into the store (dedupe by
  `dir`), set active, build the active `Project`, persist, return the entry. Rejects a
  folder whose `tutti.toml` is missing/invalid or whose forge cannot be built (e.g. gitea
  without a login).
- `switch_project(dir) -> ()` - **error if a run is active**; find the entry, build the
  forge, set active in state + store, persist. (The frontend then calls `get_board`.)
- `remove_project(dir) -> ()` - drop from the store; if it was active, clear the active
  `Project` (and `active`). Persist.

The existing `load_project` command is replaced by `add_project`/`switch_project`. The
board/run commands (`get_board`, `get_issue`, `start_run`, `pause_run`) are unchanged; they
operate on the active `Project`.

Building the forge from a `ProjectEntry` reuses the increment-1 `build_forge` (a copy of the
CLI's `wire::build`).

## Frontend (Svelte)

- **Sidebar** renders the real list from `list_projects`: each project a row (forge-colored
  dot + name), the active one highlighted; clicking a row calls `switch_project(dir)` then
  `get_board`. A small remove affordance per row (e.g. an x on hover) calls `remove_project`.
  `+ Add project` picks a folder and calls `add_project` then `get_board`. Switching is
  disabled (with a hint) while a run is active.
- **On launch**, `+page` calls `list_projects`; renders the sidebar; if `active` is set,
  switches to it and loads its board, so the app reopens where you left off.
- The add-flow's manual `owner/repo` fallback (for a folder with no git remote) from
  increment 1 is preserved in `add_project` (optional `repo` argument).

## Testing

- **Rust (hermetic):** the store read/write round-trip (serialize/deserialize `ProjectEntry`
  list + active, upsert-dedupe by dir, remove) as pure functions over an in-memory/temp-file
  store, testable without Tauri. Put the store logic (load/save/upsert/remove) in a small
  module or in `tutti-app-core` so it runs in the fast gate; keep the `AppHandle`/path
  plumbing in `src-tauri`. The switch-blocked-while-running guard is a simple state check.
- **Frontend:** a small test if the sidebar gains non-trivial logic; otherwise the existing
  store-reducer test suffices. Manual dev-run for the switching UX.

## Scope boundaries (out of increment 2)

- Browsing a forge to add existing projects (increment 3) and creating a new repo/group
  (increment 4).
- Concurrent runs across projects (one active run at a time).
- Orchestrator chat / subsessions (later slices).

## Open questions and risks

- **App data dir availability.** `app_data_dir()` should exist on macOS; create it if
  missing before the first write.
- **A persisted project that later becomes invalid** (folder moved, `tutti.toml` removed).
  `list_projects` returns entries as stored; a failed `switch_project`/`get_board` surfaces
  the error and the user can remove the stale entry. Do not silently drop entries on read.
