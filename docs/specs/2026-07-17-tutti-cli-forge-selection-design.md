# Tutti: design (CLI forge selection)

Date: 2026-07-17
Status: implemented. All three forges (GitHub, Gitea/Codeberg, GitLab) are now selectable
from the CLI via `[forge].kind` in `tutti.toml` or the `--forge` flag.
Builds on: slice 3B (three forge adapters on `main`: `tutti-forge-github`, `tutti-forge-gitea`, `tutti-forge-gitlab`).

## What this slice is

Three forge adapters exist and each is live-validated, but the CLI can only drive GitHub:
`tutti-cli/src/wire.rs` builds a concrete `GitHubForge`, and `main.rs` calls the inherent
`recover_stale` on it. This slice makes `tutti run` select any of the three forges at
runtime, so the engine can drive a GitHub, Gitea/Codeberg, or GitLab project end to end.

The engine is already dynamic over the forge (`Engine.forge: &'a dyn Forge`), so the work
is small and contained to `tutti-core::traits`, `tutti-core::config`, and `tutti-cli`.

## Decisions

- **`Box<dyn Forge>` dispatch.** `wire::build` returns a boxed trait object chosen at
  runtime. The engine already accepts `&dyn Forge`, so nothing in the engine changes.
- **`recover_stale` moves onto the `Forge` trait** as a default no-op method
  (`async fn recover_stale(&self) -> Result<()> { Ok(()) }`). The three real adapters
  override it by moving their existing inherent impls into the trait impl; `FakeForge` and
  any future forge inherit the harmless default. This is the one change that unblocks
  calling `recover_stale` through a `Box<dyn Forge>`.
- **Forge selection is config-first with CLI override.** A `[forge]` section in
  `tutti.toml` is the reproducible per-project default; `--forge` and `--login` flags on
  `tutti run` override it per invocation. Precedence: CLI flag > config > built-in default.
- **Default is GitHub.** With no `[forge]` section and no `--forge` flag, the CLI builds a
  `GitHubForge`, so every existing `tutti.toml` (including the live sandbox's) keeps
  working unchanged.

## Config and flags

New `[forge]` section (all fields optional):

```toml
[forge]
kind = "gitlab"            # "github" (default) | "gitea" | "gitlab"
login = "icesight-engine"  # the tea login; required for gitea, ignored otherwise
```

- `kind` selects the adapter. Parsed into a `ForgeKind` enum (`GitHub`/`Gitea`/`GitLab`);
  an unknown string is a load-time config error.
- `login` supplies the Gitea/Codeberg `tea` login (which encodes the host). GitHub uses
  the ambient `gh` auth and GitLab the ambient `glab` token, so neither needs a login here.

New `tutti run` flags, both optional, both overriding the config when present:

- `--forge <github|gitea|gitlab>`
- `--login <name>`

The existing `--repo` and `--repo-root` are unchanged. `--repo` is the generic target
identifier: `owner/name` for GitHub and Gitea, a numeric project id or URL-encoded path
for GitLab. `--repo-root` is the on-disk checkout where worktrees are created (all forges
use `git` for branch push).

Validation: if the resolved kind is `gitea` and no `login` is available (neither
`[forge].login` nor `--login`), `tutti run` fails fast with a clear error, since the `tea`
adapter cannot authenticate without it.

## Wiring

`wire::LiveAdapters.forge` becomes `Box<dyn Forge>`. `wire::build` takes the resolved
`ForgeKind` and optional login, and constructs:

- `GitHubForge { repo, status_labels, repo_root }`
- `GiteaForge { repo, login, status_labels, repo_root }`
- `GitLabForge { project: repo, status_labels, repo_root }`

`main.rs` resolves the effective kind/login (CLI over config), builds the adapters, calls
`adapters.forge.recover_stale().await`, and passes `adapters.forge.as_ref()` to
`Engine::new`.

## Testing

- **Hermetic (required):** `ForgeKind` parsing (each kind, the default when absent, an
  unknown-kind error) in `config` tests. A `wire` test that each kind builds a boxed forge
  without panic, and that the gitea-without-login case is rejected. The existing per-adapter
  live tiers already cover runtime behavior; this slice adds no new live tier.

Because `Box<dyn Forge>` hides concrete fields, the `wire` tests assert on the selection
path (kind resolution, login requirement) rather than peeking at adapter fields. The
current `build_uses_config_labels_and_repo` test, which reads `a.forge.repo`, is replaced
by kind-selection tests.

## Out of scope

- The Tauri UI (a later slice) that will also consume this selection.
- Per-forge base-URL / self-hosted-host configuration beyond what the `tea` login and the
  `gh`/`glab` ambient auth already provide.
- Follow-up #6 (milestone floor + planner-directed placement).

## Open questions and risks

- **Gitea login discovery.** The `tea` login name encodes the host; a wrong or missing
  login fails at the first `tea api` call. The fast-fail validation above catches the
  missing case; a wrong-host login surfaces as a forge error at run time.
- **GitLab `--repo` form.** A URL-encoded path (`group%2Fproject`) vs a numeric id both
  work with `glab api`; a raw `group/project` with an unencoded slash does not. Document
  this in the flag help.
