# Tutti: design (slice 3B, glab + tea adapters + unified status)

Date: 2026-07-14
Status: approved design (spike resolved), pre-implementation
Builds on: slice 3A (GitHub tracking + agent-driven planner), on `main` (#7, #8).

## What this slice is

Slice 3A shipped the tracking hierarchy and the agent-driven planner against GitHub
only. Slice 3B implements the other two forges the design promises, GitLab (`glab`) and
Gitea/Codeberg (`tea`), against the same `Forge` trait, and lifts one thing 3A left
GitHub-specific into shared core: the issue-status mechanism.

## Decisions (resolved during the 3B spike, 2026-07-13/14)

- **`tea` (Gitea/Codeberg).** Drive `tea <entity> --output json --repo <repo>`. `json` is
  a supported output format; `--repo` is required. The local `tea` is authed (Codeberg
  login `icesight-engine`), so tea's live tier can run now against a throwaway Codeberg
  repo.
- **`glab` (GitLab).** Drive `glab api` (REST v4 and `graphql`), JSON. `glab` is installed
  (1.107.0) but not authenticated on this machine, so glab ships hermetic (fixture-tested)
  with its live tier written but ignored until `glab auth login`.
- **Epics: try hard per forge, degrade honestly.**
  - GitHub: native sub-issues (done in 3A).
  - GitLab: native **group-level** epics via `glab api graphql`. A project belongs to a
    group; epics live on the group. Resolve the project's parent group and operate there;
    if the project has no group (personal namespace), return `EngineError::Unsupported`
    for epic operations and fall back to milestones.
  - Gitea/Codeberg: no native epic. Emulate with an `epic:<slug>` label plus a tracking
    issue. Children are the issues carrying the label; progress is computed from them.
- **Unified status abstraction (the one core change).** 3A hardcoded the
  `ready`/`in-progress`/`done` label triple inside `GitHubForge`. Lift it into a shared
  `Status` concept in `tutti-core` so every label-based forge reuses one mechanism and
  GitLab can override it with native status. See the next section.
- **`EngineError::Unsupported`** (added in 3A) is the honest-degradation signal for genuine
  gaps (GitLab epics without a group; any tracking primitive a forge lacks).

## Unified status abstraction

Today `claim`/`release`/`record` on `GitHubForge` add/remove three configured label
strings. 3B generalizes this to a `Status` the engine speaks, mapped per forge:

- **Core.** A `Status` enum in `tutti-core`: `Ready`, `InProgress`, `Done`. The engine and
  the label config are expressed in terms of `Status`, not raw strings. A
  `StatusLabels { ready, in_progress, done }` config (the existing three fields, grouped)
  maps each `Status` to a label name for the label-based forges.
- **GitHub / Gitea / Codeberg.** Identical **label** mechanism: to set a status, add its
  label and remove the other two. GitHub keeps `gh issue edit --add-label/--remove-label`;
  tea uses `tea issues edit ... --add-labels/--remove-labels` (or the label API), same
  semantics. This is a shared helper the two label-based adapters call, not duplicated
  logic.
- **GitLab.** Native issue **status** (issues are work items) via
  `glab api graphql` `workItemUpdate(input: { id, statusWidget: { status: "..." } })`.
  Native status is a higher-tier / version-gated GitLab feature and its API support has
  had gaps (gitlab work_items #584730), so glab **falls back** to a `status::<name>`
  **scoped label** (GitLab scoped labels are mutually exclusive by prefix, which mirrors
  the ready->in-progress->done transition exactly) when the native mutation is
  unavailable. The exact accepted status names are verified once `glab auth login` is done;
  until then the scoped-label path is the tested default.

This refactor is behavior-preserving for GitHub (the emitted `gh` calls are unchanged) and
is the first thing that lands in 3B, so both new adapters build on it.

## Forge trait

No new trait methods. 3B implements the existing `Forge` surface (from 3A) for two more
adapters. The status change is internal: `claim`/`release`/`record` keep their signatures;
what changes is that the label triple becomes a shared `StatusLabels`/`Status` helper
rather than GitHub-private fields.

Pure JSON parsing stays separated from shelling (as in slices 2 and 3A) so every adapter's
`glab`/`tea` output parsing is unit-tested against captured fixtures.

## Plan split (three focused PRs, never squashed)

1. **3B-status** (DONE, this branch): the unified `Status` + `StatusLabels` in `tutti-core`,
   a shared label-transition helper, and the GitHub adapter retrofitted onto it
   (behavior-preserving). Hermetic only. Small, so the two adapters build on a stable core.
   `FakeForge` and the CLI wiring were moved onto the same mapping; a `[status]` config
   section (defaulted to the `status:*` convention) makes the label triple overridable.
2. **3B-tea** (DONE): the Gitea/Codeberg adapter, crate `tutti-forge-gitea`, driven by
   `tea api` (the Gitea REST v1 passthrough, NOT the lossy `tea <entity> --output json`
   projection). Milestones + `epic:<slug>` degradation + `create_issue` + the label-based
   status via the shared `StatusLabels` helper. Gitea issue-create and label add/remove
   take numeric label IDs, so the adapter resolves label names to IDs. Parser fixtures
   captured from Codeberg + an opt-in live tier that PASSED against `workslocally/tutti-tea-sandbox`.
   CLI forge-selection is DEFERRED to a post-glab wiring pass (a shared `dyn Forge`/enum
   refactor that also owns the inherent `recover_stale`), so `tutti-cli` is untouched here.
3. **3B-glab**: the GitLab adapter (`glab`), native group epics + native/scoped-label
   status + `create_issue`. Parser fixtures; live tier written but `#[ignore]`d pending
   `glab auth login`.

Each is a separate PR merged (never squashed) before the next starts.

## Testing tiers

- **Hermetic (required CI):** all new `glab`/`tea` JSON parsers against captured fixtures;
  the `Status`/`StatusLabels` mapping and the shared label-transition helper against unit
  tests; `FakeForge` continues to model status in memory.
- **Live (opt-in, per forge):** behind the `live` feature. tea runs against a throwaway
  Codeberg repo. glab is written but ignored until authenticated. Never in required CI.

## Out of scope

- The Tauri UI (slice 4).
- Follow-up #6 (earliest-open-milestone floor + planner-directed placement).
- GitLab iterations / time-boxing beyond milestones and epics.
- Bitbucket or any fourth forge.

## Open questions and risks

- **GitLab native status names.** The exact `status` string values accepted by
  `statusWidget` depend on the instance's configured statuses and tier. Unverifiable until
  `glab auth login`; the scoped-label fallback (`status::ready` etc.) is the tested default
  and does not depend on tier.
- **GitLab group resolution.** Resolving a project's parent group for epics needs the group
  path; a personal-namespace project has none, which is the `Unsupported` case.
- **tea label API surface.** Confirm the exact `tea` subcommand for add/remove label on an
  issue during the live spike; if it lacks add/remove semantics, fall back to the labels
  API via `tea` or set the full label set each transition.
- **Fixture fidelity.** As in 3A, capture real `glab api`/`tea --output json` shapes during
  the build rather than hand-writing fixtures, so the parsers are tested against reality.
