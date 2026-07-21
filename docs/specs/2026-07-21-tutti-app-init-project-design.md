# Tutti: design (desktop app - initialize an existing local project)

Date: 2026-07-21
Status: approved design (scope confirmed), pre-implementation
Builds on: desktop app increment 2 (multi-project switching) on branch `app-multiproject`.

## What this is

Increment 2's "Add project" only accepts a folder that already has a `tutti.toml`. Most
real repos do not. This adds the front half of the onboarding wizard: when you pick a
local git repo with no `tutti.toml`, the app offers a short **Initialize** form that writes
a `tutti.toml`, seeds the `status:*` labels in the forge, and adds the project so it is
immediately runnable.

Creating a brand-new repo/group in the forge (increment 4) is still separate; this only
onboards an existing LOCAL folder.

## Decisions (confirmed)

- **Guided form** (not one-click, not full create-in-forge): a few fields with smart
  defaults, forge kind auto-detected from the git remote host.
- **Seed the status labels** in the forge as part of init, so the repo can run right away.

## Flow

1. In the sidebar "Add project" flow, after the folder is picked, the app **probes** it.
2. If it has a `tutti.toml`, add it as today (`add_project`).
3. If not (but it is a git repo), show the **Initialize** form, pre-filled:
   - forge kind (auto-detected from the remote host, editable),
   - repo slug (auto-detected),
   - login (shown only for gitea),
   - integration branch, model, gate command (sane defaults, editable).
4. Submit -> write `tutti.toml`, create the missing `status:*` labels in the forge, activate
   and persist the project.

## Backend (Tauri + core)

- **`tutti-app-core` (pure, tested):**
  - `forge_kind_from_remote(url) -> Option<String>`: host mapping (github.com -> github,
    gitlab.com -> gitlab, codeberg.org -> gitea; other hosts -> None, user picks).
  - `render_tutti_toml(params) -> String`: generate a valid `tutti.toml` from the form
    params. A round-trip test asserts `Config::load` accepts the output. Defaults:
    `trunk = "main"`, `routing = "trunk"`, `integration_branch = "staging"`,
    `model = <a sensible default>`, `max_issues_per_run = 25`,
    `[select] require_label = "status:ready"`, `skip_labels = ["status:needs-human"]`,
    `[gate] commands = ["true"]`, `[forge] kind = <detected>` (+ `login` for gitea).
- **`Forge::create_label(name, color) -> Result<()>`** (new trait method), tolerant of an
  already-existing label, implemented for GitHub (`gh label create ... --force`), Gitea
  (`POST labels`, tolerate 409), GitLab (`POST labels`, tolerate exists), and FakeForge
  (no-op). Used to seed `status:ready` / `status:in-progress` / `status:done` /
  `status:needs-human`.
- **Commands (`src-tauri`):**
  - `probe_project(dir) -> { has_config, repo: Option<String>, forge_kind: Option<String> }`
    - checks for `tutti.toml`, detects the repo slug and forge kind from the git remote.
  - `init_project(dir, params, state) -> ProjectEntry` - render + write `tutti.toml`, build
    the forge, `create_label` the four status labels (tolerant), then activate + persist
    (reusing the increment-2 `activate` + store upsert). Blocked while a run is active,
    like add/switch.

## Frontend (Svelte)

- The sidebar's folder-pick success handler calls `probe_project`. If `has_config`, it
  calls `addProject` as now. Otherwise it opens an **Initialize** sub-form (in the sidebar
  add area) pre-filled from the probe: forge kind (a small select), repo, login (gitea
  only), integration branch, model, gate command. Submit calls `init_project`, then
  refreshes the list and activates the new project (same as add).
- If the folder is not a git repo at all (no remote and no config), fall back to the
  existing manual `owner/repo` entry plus the forge fields (so a bare folder can still be
  initialized by typing the slug).

## Testing

- **Rust (hermetic):** `forge_kind_from_remote` mapping; `render_tutti_toml` -> `Config::load`
  round-trip (the generated file parses and validates); `create_label` is a no-op on
  FakeForge (the adapter shell calls are covered by the existing live tiers, not the gate).
- **Frontend:** existing store-reducer test suffices; manual dev-run for the init flow.

## Scope boundaries

- Creating a new repo/project/group in the forge (increment 4).
- Detecting/generating a real gate command from the repo's build system (defaults to
  `["true"]`; the user edits it).
- Editing an existing `tutti.toml` from the app (out; edit the file directly).

## Open questions and risks

- **Label color defaults.** Seed with reasonable colors (ready green, in-progress amber,
  done blue, needs-human red); the user can recolor in the forge.
- **A folder with no git remote.** `probe_project` returns `repo: None`; the form requires a
  manual slug (and forge kind) before init.
- **Gitea login.** Required for the gitea adapter; the form must collect it (as the add
  flow's fallback does) or init fails at forge build.
