# Browse a forge to add an existing project (app increment 3)

Status: designed

## Problem

The only way to add a project is to point at a folder that is already cloned. If the
repo lives on a forge you have access to but not on this machine, the app cannot help:
you leave, clone it by hand, come back, and pick the folder.

## Goal

Add the second entry point: browse the forges you are authenticated to, pick a repo,
clone it, and land in the existing wizard with everything already known.

## Why this cannot hang off the `Forge` trait

Every `Forge` implementation is constructed **with** a repo (`GitHubForge { repo, .. }`,
`GitLabForge { project, .. }`, `GiteaForge { repo, .. }`), because everything the trait
does is scoped to one. Browsing happens strictly before a repo exists, so it needs its
own repo-independent capability that depends on nothing but the CLI and, for Gitea, the
login.

That is a new trait in `tutti-core`, a sibling of `Forge` rather than part of it:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceKind { User, Org, Group }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    /// The path the repo listing is keyed by: a login, an org name, or a group path.
    pub path: String,
    /// Human-readable name for the picker; falls back to `path`.
    pub name: String,
    pub kind: NamespaceKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRepo {
    /// `owner/repo`, or `group/subgroup/project` on GitLab. This is exactly the string
    /// the resulting `tutti.toml` records, so it must match what the `Forge` adapter
    /// expects.
    pub full_path: String,
    pub name: String,
    pub description: Option<String>,
    pub clone_url: String,
    pub private: bool,
}

#[async_trait]
pub trait ForgeBrowser: Send + Sync {
    /// The namespaces the authenticated user can see: their own account, plus the orgs
    /// or groups they belong to.
    async fn list_namespaces(&self) -> Result<Vec<Namespace>>;
    /// The repos in one namespace, newest first.
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>>;
}
```

Each adapter crate gains a browser type alongside its forge:
`GitHubBrowser`, `GitLabBrowser`, `GiteaBrowser { login }`.

### The CLI calls

All through the CLIs already in use, so authentication is whatever the user already has.

| Forge | Namespaces | Repos |
|---|---|---|
| GitHub | `gh api user`, `gh api user/orgs --paginate` | `gh api users/{login}/repos --paginate`, `gh api orgs/{org}/repos --paginate` |
| GitLab | `glab api user`, `glab api groups?min_access_level=30&per_page=100` | `glab api groups/{id}/projects?per_page=100`, `glab api users/{id}/projects?per_page=100` |
| Gitea | `tea api user`, `tea api user/orgs` (with `--login`) | `tea api users/{login}/repos`, `tea api orgs/{org}/repos` |

Notes that shape the implementation:

- **Pagination is not optional.** A GitHub account with hundreds of repos, or a GitLab
  instance with a deep group tree, will silently truncate at the default page size and
  the missing repo will look like a permissions problem. GitHub and Gitea use
  `--paginate` where available; GitLab gets an explicit page loop capped at 10 pages
  (1000 repos), and the UI states when the cap was hit rather than quietly truncating.
- **GitLab namespaces are keyed by numeric id, not path**, for the projects call. The
  `Namespace` therefore carries the id in `path` for groups and the UI shows `name`.
  For the user's own namespace GitLab wants `users/{id}/projects`.
- **Gitea needs `--login`** on every call, exactly like `GiteaForge`, and the flag must
  precede the endpoint positional (urfave-cli v1 rejects it otherwise). This is a
  gotcha already learned in 3B-tea and it applies unchanged here.

## The flow

`+ Add project` currently opens a folder picker immediately. It becomes a choice:

```
+ Add project
 ├─ Open a local folder...        (today's path, unchanged)
 └─ Browse a forge...             (new)
```

Browse opens a modal with its own steps, reusing `QuestionCard` so it looks like the
wizard it hands off to:

1. **Which forge?** The same three radio cards as the wizard, plus the Gitea login
   field. There is nothing to detect here, so unlike the wizard this step always shows.
2. **Which namespace?** A list of the user's account plus their orgs or groups, with a
   filter box. Loading and error states inline; an error here is almost always "the CLI
   is not authenticated", so the message says which CLI and what to run.
3. **Which repo?** A searchable list showing name, description and a private marker.
   Filtering is client-side over the fetched page set.
4. **Where should it go?** A parent-directory picker. The clone target is
   `<parent>/<repo name>`, shown explicitly before you commit to it.

On confirm the backend clones, then probes the result and reuses what already exists:

- Target path already exists and is a git repo: skip the clone and use it. This is the
  common "I already cloned this" case and failing on it would be obnoxious.
- Target path exists and is not a git repo: error, do not touch it.
- Otherwise `git clone <clone_url> <target>`.
- Then `probe_project(target)`: has a `tutti.toml` and the project is added directly;
  no config and the existing wizard opens with folder, forge and repo all known, which
  by `stepsFor` means just **trunk, routing, model, review**.

Cloning blocks with the button reading "Cloning...". No progress reporting in this
increment: `git clone` gives no machine-readable progress without extra plumbing, and a
spinner that cannot lie is better than a progress bar that can.

## Code structure

**Core** (`crates/tutti-core/src/browse.rs`, new): `Namespace`, `NamespaceKind`,
`RemoteRepo`, `ForgeBrowser`. Re-exported from `lib.rs`. A `FakeBrowser` lands in
`tutti-core::testing` next to `FakeForge` so app-core assembly can be tested hermetically.

**Adapters:** `browse.rs` in each of the three forge crates, holding the browser type,
and a `parse` module (or additions to the existing one) for the pure JSON to type
mapping. The parsers are the risky part and stay separate from the shelling so they can
be tested against captured fixtures, matching how `parse.rs` already works.

**Backend** (`tutti-app/src-tauri/src/commands.rs`): four commands.

- `list_namespaces(forge_kind, login) -> Vec<Namespace>`
- `list_repos(forge_kind, login, ns) -> Vec<RemoteRepo>`
- `clone_repo(clone_url, parent_dir, name) -> String` returning the target path, with
  the already-exists handling above
- these are all run-guarded like the other project-mutating commands

A private `build_browser(kind, login)` mirrors the existing `build_forge`.

**Frontend:** `BrowseForge.svelte` (the modal), `browse.ts` (pure: the step list, the
filter predicate, target-path derivation, validation), `browse.test.ts`. `Sidebar.svelte`
gains the two-way choice; `+page.svelte` owns the browse modal the same way it owns the
wizard, and wires its completion into the existing `onAdd` / wizard handoff.

## Testing

- **Fixture parsers.** One captured JSON payload per forge per call (namespaces, repos),
  recorded from the real CLIs, asserted into the exact `Namespace` / `RemoteRepo` values.
  Includes a GitLab payload with a nested subgroup and a GitHub payload containing a fork
  and an archived repo, since those are the shapes most likely to be mishandled.
- **Live tier** behind the existing `live` feature, one per adapter, asserting the user's
  own namespace appears and that the sandbox repo is listed under it. Validated against
  the real GitHub, GitLab and Codeberg accounts before merge, following the 3B pattern
  where the live tier is what actually catches wire-format divergence.
- **Clone handling** unit-tested through a small pure helper for the target-path and
  already-exists decisions, so the branchy part is not buried in an IO function.
- **Frontend** `browse.test.ts` over the pure module, plus the existing gates.

## Out of scope

- Creating a repo, group or org. That is increment 4.
- Repos on a host the CLI is not authenticated to. The error says so; there is no
  in-app login flow.
- Clone progress, shallow clones, and submodules.
- Converting the cloned repo's existing backlog into something the engine can pick up.
  That is issue #16, and it is a real gap: a browsed-and-cloned repo with an untriaged
  backlog will show a full READY column that the engine will not touch.
