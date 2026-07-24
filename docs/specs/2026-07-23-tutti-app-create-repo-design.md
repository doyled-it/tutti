# Create a new repo from scratch (app increment 4)

Status: implemented

## Problem

The app can add a project two ways: open a local folder (with the init wizard for a
repo that predates Tutti), and browse a forge to clone an existing remote (increment 3,
#17). The missing onboarding path is the from-nothing one: you have no repo yet. Today
you leave the app, create the repo on the forge by hand, clone it, come back, and add
the folder.

## Goal

Add the third entry point: create a brand-new repo on a forge you are authenticated to,
clone it, and land in the existing new-project wizard with the forge and repo already
known. Create + clone + hand off. Nothing more.

## Scope and non-goals

- **In scope:** create one repo under an existing namespace (your account, an org, or a
  group), auto-initialized so the immediate clone lands cleanly, then the existing
  wizard configures it.
- **Non-goal:** creating a brand-new org or group. That has no clone and no wizard
  handoff, so it is a different feature. The namespace picker only ever *targets*
  existing namespaces.
- **Non-goal:** deleting a repo from the app. Creation is the only product capability
  added here (see Testing for why deletion lives only in the test harness).

## Why this hangs off `ForgeBrowser`, not `Forge`

Every `Forge` implementation is constructed **with** a repo, because everything the
trait does is scoped to one. Creating a repo happens strictly before a repo exists, so
it belongs on the repo-independent sibling trait `ForgeBrowser`, alongside
`list_namespaces` / `list_repos` (increment 3). It depends on nothing but the CLI and,
for Gitea, the login: exactly what `ForgeBrowser` already carries.

## Core: one new trait method

```rust
/// A repo to create. Always auto-initialized with a README so the immediate clone
/// lands on a real default branch rather than an unborn one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRepo {
    pub name: String,
    pub description: Option<String>,
    pub private: bool,
}

#[async_trait]
pub trait ForgeBrowser: Send + Sync {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>>;
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>>;
    /// Create `spec` under `ns` and return it in the same shape `list_repos` yields,
    /// so its `clone_url` feeds the existing clone path unchanged.
    async fn create_repo(&self, ns: &Namespace, spec: &NewRepo) -> Result<RemoteRepo>;
}
```

Auto-init is unconditional, not a `NewRepo` field: an empty repo clones to an empty dir
on an unborn branch with no forge-set default branch, which the wizard's label-seeding
and first commit implicitly assume against. Always initializing with a README removes
that whole class of edge case; the small cost is a `README.md` the user can edit later.

`FakeBrowser` gets a `create_repo` that records the call and returns a synthesized
`RemoteRepo`, for hermetic tests of the backend and the handoff.

## Adapters: create by `NamespaceKind`

Each adapter branches the create call on `ns.kind` and parses the response into a
`RemoteRepo` with the same field rules its `list_repos` already uses.

### GitHub (`gh`)

`gh repo create <ns.path>/<name> --private|--public --add-readme` plus
`--description <d>` when set. Org vs user is purely the `owner/` prefix on the name;
there is no endpoint change. `gh repo create` emits little useful JSON, so **read the
repo back** with `gh repo view <ns.path>/<name> --json name,description,url,isPrivate,sshUrl`
(or `gh api repos/<ns.path>/<name>`) to build the `RemoteRepo`. Visibility is a flag,
not a field. `--add-readme` is the auto-init.

### Gitea / Codeberg (`tea api`)

Different **endpoints** by kind, not just a field:

- User: `POST /user/repos`
- Org: `POST /orgs/{ns.path}/repos`

Body `{ "name": ..., "private": <bool>, "auto_init": true, "description": ... }`. The
response is raw Gitea JSON (`clone_url`, `private`, `description`, `full_name`), the same
shape `list_repos` parses. Keep the 3B-tea invariants: `--login <l>` **precedes** the
endpoint positional (urfave v1), and the arg list is built variable-free (shell var
expansion mangles the flags).

### GitLab (`glab api`)

Single endpoint, namespace selected by an optional numeric id:

`POST /projects` with `-f name=<name> -f visibility=private|public -f initialize_with_readme=true`,
plus `-f namespace_id=<ns.path>` for a Group (on GitLab `Namespace.path` is already the
numeric id) and omitted for User. `-f description=<d>` when set. The response gives
`path_with_namespace` (→ `full_path`), `http_url_to_repo` (→ `clone_url`), and
`visibility` (→ `private = visibility != "public"`, the same rule browse uses, so
`internal` is correctly not-public). Uses the stored token; no `--login` flag.

## Backend command

```rust
#[tauri::command]
pub async fn create_repo(
    forge_kind: String,
    login: Option<String>,
    namespace: Namespace,
    spec: NewRepo,
    state: tauri::State<'_, AppState>,
) -> Result<RemoteRepo, String>;
```

Run-guarded like `list_namespaces` / `list_repos` / `clone_repo` ("pause the run before
creating a repo"). It builds the browser with the existing `build_browser(kind, login)`
and calls `create_repo`. It does **not** clone: the frontend takes the returned
`clone_url` and calls the existing `clone_repo` command, so there is one clone
implementation, not two.

## Frontend

A new `CreateRepo.svelte` modal mirroring `BrowseForge.svelte`'s shell exactly: scrim,
header with step dots, Back/Next footer, Escape to cancel, Tab focus-trap, focus-first-
control on step change, and the generation-token guard on the namespace load.

Steps:

1. **Forge**: same radio group as browse (GitHub / GitLab / Gitea, with the tea login
   sub-field for Gitea). Reuses `validateBrowseStep` for the Gitea-login rule.
2. **Target namespace**: reuses the increment-3 `list_namespaces` load and picker
   verbatim (account, orgs, groups).
3. **Repo details**: name (required, validated), visibility (private default), optional
   description.
4. **Destination**: same parent-folder picker and `cloneTarget` preview as browse.
5. **Create & clone**: call `create_repo`, then `clone_repo(returned.clone_url,
   parentDir, returned.name)`, then `onCloned(path, forgeKind, login)`.

Pure helpers live in `create.ts` (no Svelte/Tauri imports), unit-tested with vitest,
following the `browse.ts` convention:

- `createSteps()`: the step id list.
- `validateName(name)`: non-empty, no whitespace or path separators, forge-legal
  characters; returns an error string or null.
- `validateCreateStep(state, step)`: per-step gate (delegates the forge step to
  `validateBrowseStep`).

The Sidebar "+ Add project" popover gains a third entry, **"Create a new repo…"**,
beside "Open a local folder…" and "Browse a forge…", handed off to a page-owned
`CreateRepo` modal the same way `beginBrowse` opens `BrowseForge`.

## Handoff into the wizard

Reuses `onCloned(dir, forgeKind, login)` verbatim. A freshly created repo has no
`tutti.toml`, so `probeProject` reports `has_config = false` and the flow deterministically
lands in `InitWizard` with the forge kind and (for Gitea) the login pre-known, exactly
as the browse clone does. No new handoff code, and the wizard's own README (from
auto-init) plus its later tutti.toml write and label seeding proceed normally.

## Testing

### Hermetic (default gates)

- `FakeBrowser::create_repo` drives the backend command and the reducer/handoff paths.
- One `create.rs` fixture parse test per adapter, from a **real captured create
  response**, asserting the returned `RemoteRepo` fields (`full_path`, `clone_url`,
  `private`, `description`).
- `create.ts` vitest: `validateName` edge cases, `validateCreateStep` per step,
  `createSteps` shape.

### Live tier (opt-in `live` feature)

Create + clone + **delete-cleanup via a raw CLI call in the harness** (not a trait
method):

- **GitHub**: `doyled-it`, cleanup `gh repo delete <path> --yes`.
- **Gitea / Codeberg**: the `doyled-it` namespace (resolve the `tea` login via
  `tea api user` rather than assuming login == namespace), cleanup `tea api --login <l>
  -X DELETE repos/<path>`.
- **GitLab personal**: `doyled-it` on gitlab.com, cleanup `glab api -X DELETE
  projects/<url-encoded-path>`.

GitLab **group**-create stays hermetic: it is the same `POST /projects` endpoint with
only `namespace_id` added, which the fixture test covers, and a throwaway group is not
worth standing up. Deletion is deliberately kept out of `ForgeBrowser` so no destructive
capability ever reaches the UI; the harness owns cleanup with the same CLIs the adapters
drive.

## Wire-format gotcha summary

| Forge | Create call | Namespace selector | Auto-init | Response read |
|-------|-------------|--------------------|-----------|----------------|
| GitHub | `gh repo create owner/name --private --add-readme` | `owner/` name prefix | `--add-readme` | read back via `gh repo view --json` |
| Gitea | `POST /user/repos` or `/orgs/{org}/repos` | different endpoint by kind | `auto_init:true` | raw Gitea JSON, `--login` before endpoint |
| GitLab | `POST /projects` | `namespace_id` (numeric, group only) | `initialize_with_readme=true` | `path_with_namespace` / `http_url_to_repo` / `visibility` |

## Files touched

- `crates/tutti-core/src/browse.rs`: `NewRepo`, `create_repo` on `ForgeBrowser`.
- `crates/tutti-core/src/testing/fake_browser.rs`: fake `create_repo`.
- `crates/tutti-forge-{github,gitea,gitlab}/src/browse.rs`: adapter impls + fixtures.
- `tutti-app/src-tauri/src/commands.rs`: `create_repo` command.
- `tutti-app/src-tauri/capabilities/default.json`: no change (no new plugin).
- `tutti-app/src/lib/ipc.ts`: `createRepo` binding, `NewRepo` type.
- `tutti-app/src/lib/create.ts` (+ `create.test.ts`): pure helpers.
- `tutti-app/src/lib/components/CreateRepo.svelte`: the modal.
- `tutti-app/src/lib/components/Sidebar.svelte`: third add-project entry.
- `tutti-app/src/routes/+page.svelte`: own the `CreateRepo` modal, reuse `onCloned`.
