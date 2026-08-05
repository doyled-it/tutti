# Tutti: design (slice 3, forge tracking + real planning)

Date: 2026-07-11
Status: approved design, pre-implementation
Builds on: slices 1 (offline core) and 2 (live adapters), both on `main`.

## What this slice is

Slice 3 teaches Tutti about the tracking hierarchy above a single issue (milestones,
epics, roadmaps) and turns the planner from a deterministic stub into a real,
agent-driven orchestrator that can act on that hierarchy. It targets all three forges:
GitHub (`gh`), GitLab (`glab`), and Gitea/Codeberg (`tea`), with a common abstraction
that degrades gracefully where a forge lacks a primitive.

## Decisions

- **GitHub epics = native sub-issues.** GitHub exposes a real issue hierarchy: the
  `repos/{o}/{r}/issues/{n}/sub_issues` REST endpoint, a `sub_issues_summary`
  (completed/total/percent) on every issue, and an issue `type` field. An epic is a
  parent issue with sub-issues; progress is the summary. No label hacks, no Projects v2
  GraphQL.
- **All three forges.** GitHub, GitLab, Gitea/Codeberg. The abstraction is common; each
  adapter maps it to its primitives and reports what it cannot do.
- **Agent-driven planner.** After a drain, the engine runs the Planner role via the
  backend with the live tracking state in its prompt, and executes the whitelisted
  actions it returns.
- **Auto-close milestones when verifiably drained.** When every issue in a milestone is
  done (a verifiable all-children-complete check, not the planner's say-so), the engine
  closes the milestone. This narrows slice 1's "CloseMilestone is always human-surfaced"
  guardrail to "auto-close only on a verified drain," keeping it safe while making the
  orchestrator able to close out its own work.

## The cross-forge tracking model

Common types (in `tutti-core`):
- `Milestone { id, title, state (open|closed), due: Option<Date>, progress }`
- `Epic { id, title, children: Vec<IssueId>, progress }`
- `Roadmap { lanes }` — a derived, read-only view (ordered milestones/epics over time).

Per-forge mapping:

| Concept | GitHub (`gh`) | GitLab (`glab`) | Gitea/Codeberg (`tea`) |
| --- | --- | --- | --- |
| Milestone | native (`gh api .../milestones`) | native | native |
| Epic | parent issue + native sub-issues; progress from `sub_issues_summary` | native epics (GROUP-level, see risks) | no native epic; degrade to a tracking issue with a task-list (or an `epic:<slug>` label), progress computed from the listed children |
| Roadmap | derived: open milestones ordered by `due_on` (Projects v2 deferred) | native roadmap or derived | derived from milestones |

Each adapter returns a capability signal (or an `Unsupported` error) for the parts it
degrades, so the engine and the future UI can show honest state.

## Forge trait extensions

Added to the `Forge` trait (all adapters implement; some degrade):

- `list_milestones() -> Vec<Milestone>`
- `create_milestone(title, due, description) -> Milestone`
- `close_milestone(id)`
- `milestone_children(id) -> Vec<Issue>` (to compute a verifiable drain)
- `list_epics() -> Vec<Epic>` / `epic_children(id) -> Vec<IssueId>` / `epic_progress(id)`
- `create_epic(title, ...) -> Epic` / `link_sub_issue(parent, child)`
- **`create_issue(new: NewIssue, milestone: Option<MilestoneId>, epic: Option<EpicId>) -> Issue`**
  — the method the planner's `CreateIssues` always needed and never had.
- `roadmap() -> Roadmap` (derived)

The pure per-adapter parsing (of `gh`/`glab`/`tea` JSON) is separated from the shelling,
as in slice 2, so it is unit-testable against captured fixtures.

## Engine wiring (real planning)

1. **Agent-driven planner.** `engine.plan()` stops being a stub. After a drain (shipped
   > 0, or on a cadence), the engine runs the Planner role via the backend, passing a
   compact snapshot of tracking state (open milestones with progress, open epics, ready
   issue counts) built from the new `Forge` reads. The agent returns a `PlanDecision`.
2. **Execute whitelisted actions.** The slice-1 whitelist still governs: `NextIssue`
   (continue draining) and `CreateIssues` (now real via `create_issue`, placed under the
   milestone/epic the planner names) execute automatically; anything with
   `needs_human: true` is surfaced.
3. **Milestone-aware selection.** `SelectFilter` gains an optional milestone scope so the
   engine can prefer the earliest open milestone (a soft floor, like SOTTO's
   `active_milestone`), falling through when it drains.
4. **Auto-close on verified drain.** After each ship (and in the planner step), the engine
   checks the shipped issue's milestone via `milestone_children`; if all are done, it
   calls `close_milestone`. This is gated on the verified all-done check, never on the
   planner's opinion alone.

Guardrails unchanged in spirit: the executor still never merges to trunk; the planner
still cannot spend money / touch secrets / publish; `close_milestone` is the one action
promoted from "always surface" to "auto on verified drain."

## Plan split (large slice, two verified passes; still ships all three forges)

- **Plan 3A**: tracking types + `Forge` trait extensions + the **GitHub adapter**
  (sub-issues + milestones + `create_issue`) + the engine's real-planning wiring
  (agent-driven planner, whitelisted execution, milestone-aware selection, auto-close on
  verified drain). Hermetic parser/engine tests + one live sandbox run.
- **Plan 3B**: the **`glab`** and **`tea`** adapters, handling GitLab's group-level epics
  and Gitea's epic degradation, each with parser tests and an opt-in live tier.

## Testing tiers

- **Hermetic (required CI):** all `gh`/`glab`/`tea` JSON parsers against captured
  fixtures; the engine's planner/selection/auto-close logic against fakes (a `FakeForge`
  extended with in-memory milestones/epics and a scriptable planner outcome).
- **Live (opt-in, per forge):** behind the `live` feature, end-to-end against a throwaway
  repo on each forge. Never in the required check.

## How the floor and placement actually landed

Points 2 and 3 of "Engine wiring" were deferred out of Plan 3A and delivered later
(issue #6). What shipped, and why it differs from the sketch above:

**The floor is a search order, not a scope.** `SelectFilter` gains
`milestone_floor: bool` (`#[serde(default)]`, off), and when it is on
`Engine::select_ready_issue` asks the same selector once per open milestone, earliest
first, taking the first hit. The last probe is deliberately *unscoped*, so once no open
milestone has ready work the loop still picks up issues that belong to no milestone at
all. Turning the floor on can reorder a drain; it can never starve one. An explicit
`select.milestone` wins outright, since a pinned scope already answers the same question.

**"Drained" means "no ready issue left", not `Progress::is_drained`.** The two disagree
when a milestone's last open issue is parked behind `status:needs-human`: the rollup says
work remains, but the loop cannot touch it. Using the rollup there would pin the floor to
a milestone forever. Selection asks the only question it can act on.

**Floor order is total and forge-independent.** `tracking::milestone_floor_order` sorts
open milestones by due date, undated last, `id` as the tiebreak. Without the tiebreak the
floor would follow whatever order the forge listed them in and could wobble between
iterations.

Two things this originally got wrong, both found in review:

- The comparison is a string compare, and the doc claimed that was chronological because
  dates are ISO. Only GitLab returns `YYYY-MM-DD`; GitHub and Gitea pass `due_on` through
  as RFC 3339, and Gitea with a real offset — where a text compare is simply wrong
  (`2026-08-01T00:00:00+02:00` precedes `2026-07-31T23:00:00Z` but sorts after it). The
  adapters now normalise `due` at the boundary, so `Milestone.due` matches what its type
  claims and the comparator's assumption is established rather than assumed.
- The same open-filter-then-sort-by-due already existed in all three adapters' `roadmap()`,
  feeding the UI's roadmap rail. This added a fourth copy that differed on the tiebreak, so
  the rail and the engine could disagree about which of two same-due milestones came first.
  All three now delegate to `milestone_floor_order`.

**"Closed milestones are dropped" is a claim about the order, not about selectability.**
The final probe is unscoped, so an issue left behind in a closed milestone is still
reachable. That follows from "never starve", but the original test named for this asserted
only the ordering half; there is now a test for each.

**Placement hints are titles, resolved leniently.** `NewIssue` gains
`milestone: Option<String>` and `epic: Option<String>`. Titles rather than ids because the
planner only ever sees titles, in the snapshot. `execute_plan` resolves them once per
batch (exact match, then case-insensitive, since the planner is retyping a title it read),
and an unresolvable hint files the issue at the top level rather than dropping proposed
work. It also skips `list_epics` entirely unless some issue names an epic: that read is an
N+1 on GitHub, and the snapshot does not list epics yet, so the common plan pays nothing.

**Cost of the floor:** one `list_ready_issues` plus one `list_milestones` per iteration.
The ranking is pure.

This was originally written as "one selector call per open milestone… both cheap forge
reads", which was wrong in a way that mattered. Every adapter implements selection as a
single list fetch filtered in process, so asking per milestone was M identical fetches with
M-1 discarded — on GitHub, M `gh` subprocess spawns and M API round trips to answer a
question one fetch already contained. The floor is ordering, not a query, so
`list_ready_issues` is now the `Forge` primitive (with `next_ready_issue` derived from it)
and `pick_by_milestone_floor` ranks the result purely. A `FakeForge` call counter pins the
count, because a cost claim that nothing asserts is a cost claim that drifts back.

## Out of scope

- The Tauri UI (slice 4) that will consume `roadmap()` and the tracking reads.
- GitHub Projects v2 roadmaps (derived-from-milestones is enough for now).
- Iterations / GitLab-specific time-boxing beyond milestones.

## Open questions and risks

- **GitLab epics are group-level, not project-level.** A standalone project under a group
  has epics at the group. The `glab` adapter must resolve the parent group (or degrade to
  milestones when the project has no group). Decided in Plan 3B.
- **Gitea epic degradation.** Tracking-issue-with-task-list vs `epic:` label. Task-list
  gives real children + rollup; label is simpler. Decided in Plan 3B; lean task-list.
- **Planner prompt size.** The tracking snapshot must stay compact (counts + titles, not
  full issue bodies) so the planner prompt does not balloon.
- **Auto-close verification.** The all-children-done check must read the milestone's
  issues from the forge at close time (not trust a cached count), to avoid closing a
  milestone that gained a new open issue mid-run.
- **`gh` sub-issues via `gh api` only.** The `gh` CLI has no first-class sub-issue
  subcommands; the adapter uses `gh api` for the hierarchy endpoints.
