<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Converting an existing repo into a Tutti repo (#16, steps 2 and 3)

## Where this picks up

Step 1 shipped (see `2026-07-23-tutti-app-board-untriaged-design.md`): untriaged issues
are their own board bucket instead of being smuggled into Ready, so the board no longer
claims work is ready when a Run would pick up zero of it.

That made the gap **visible**. It did not make it **closable**. Today the only way to
triage a pre-Tutti backlog is the forge's web UI or a shell loop. This slice closes it:

- **Step 2.** Show the gap as a number, let the user select untriaged issues, and apply a
  triage decision to the selection.
- **Step 3.** Let the orchestrator walk the backlog and *propose* that selection, because
  a human hand-labelling 59 issues is exactly the chore Tutti exists to remove.

Both land together because step 3 is step 2 plus a proposal that drives the same apply
path. Splitting them would mean designing the apply command twice.

## The decision that shapes everything else: two outcomes, not one

The obvious version of this feature is a "mark everything ready" button. Issue #16 argues
against it in its own text, and it is right:

> Most backlogs contain things that are not specced enough for an agent to pick up, so
> "label everything ready" is the wrong default and is exactly how you get an agent
> working on a one-line issue that says "fix the thing".

So triage has **two** outcomes, not one: mark ready, or park as needs-human. Without the
park action a triage pass cannot finish. The user reviews issue #12, decides it is too
vague, and then has nowhere to put that decision: leaving it untriaged makes it
indistinguishable from the 40 issues they have not looked at yet. Next session they start
over. One button produces a chore that never converges.

## `status:needs-human` must become a board status

This falls straight out of the above, and it is the same bug step 1 fixed, one level down.

`status:needs-human` is already a first-class engine concept: it is the default entry in
`select.skip_labels`, and GUARDRAIL #1 means the engine will never select an issue
carrying it. But `classify` only knows done / in-progress / ready, so a needs-human issue
falls through to `Status::Untriaged` and renders in the Untriaged column.

That is a lie of the same shape as the one step 1 removed. "Untriaged" means *nobody has
decided*. A parked issue is the opposite: somebody decided, and the answer was no. Showing
them identically means the park action would appear to do nothing, and the user would
re-triage the same issue forever.

**Decision:** add `Status::NeedsHuman` and a `needs_human` board bucket, rendered as its
own column with the same hide-when-empty rule Untriaged already uses.

### Classification order

`classify` gains the skip labels and resolves in this order:

    done -> in_progress -> needs_human -> ready -> untriaged

The only interesting position is **needs-human above ready**. An issue carrying both
`status:ready` and a skip label is *not selectable*: `next_ready_issue` requires the
ready label AND the absence of every skip label. Ranking ready first would put it in the
Ready column, which would be the original bug again. The board's Ready column must mean
"a Run would pick this up".

Done and in-progress stay above needs-human: a shipped issue is shipped, and an issue a
runner currently holds is in progress, whatever else it is labelled.

`classify` matches against **every** entry in `select.skip_labels`, not just the first, so
a project with `skip_labels = ["blocked", "needs-human"]` classifies both correctly.

## Forge write path

Every adapter already has this primitive inside its private `set_status`: add one label,
remove some others, in one call. `Forge` gets it as a public method:

```rust
/// Add and remove labels on an issue in one call. Removing a label the issue does not
/// carry must be a no-op, not an error (all three adapters already behave this way).
async fn edit_labels(&self, issue: IssueId, add: &[String], remove: &[String]) -> Result<()>;
```

**Why generic labels rather than a `set_triage(issue, Triage)` method.** The park label is
not derivable from anything a forge holds. Adapters are constructed with `StatusLabels`
(ready / in-progress / done); the skip labels live in `SelectFilter`, which is config the
forge never sees. A `set_triage` method would therefore need the label passed in anyway,
at which point it is `edit_labels` with a narrower name. Keeping the trait generic puts
the policy in the one layer that holds the whole `Config`.

Each adapter's `set_status` is refactored to call `edit_labels`, so there is one place per
forge that knows how to write labels rather than two that can drift.

## The apply command

```rust
#[tauri::command]
pub async fn apply_triage(
    issues: Vec<u64>,
    to: TriageTarget,          // "ready" | "needs_human"
    state: State<'_, AppState>,
) -> Result<TriageOutcome, String>
```

Three guardrails, each earning its place:

**Run-guarded.** Refused while a run is active, mirroring `apply_gate`. Relabelling
changes what the drain selects; doing it mid-run means the engine's next selection differs
from the one the user was looking at.

**Eligibility is re-read from the forge, and protects in-flight work rather than
"anything already labelled."** The command re-reads the issues and drops any that are
`InProgress` or `Done`: flipping a shipped issue back to ready hands finished work to an
agent, and the in-progress label *is* the claim lock, so stripping it could cause a
double-claim. Everything upstream of that (`Untriaged`, `Ready`, `NeedsHuman`) is eligible.

The first draft of this design restricted eligibility to `Untriaged`, which was wrong in
two ways found during review. It made parking a one-way door: an issue parked from the
board could never be un-parked from the board. And it silently broke the orchestrator path,
which proposes over the whole backlog, so a proposed park for an already-`Ready` issue
would have been accepted by the UI and then skipped by the command with no explanation.

The re-read itself is what makes the decision current rather than trusting whatever the UI
happened to be showing, which for the orchestrator path may be a read from minutes ago.

**Partial failure is reported, not swallowed.** Applying to 59 issues over a network will
sometimes fail on issue 30. The command continues and returns:

```rust
pub struct TriageOutcome {
    pub applied: Vec<u64>,
    pub skipped: Vec<u64>,              // no longer untriaged when re-read
    pub failed: Vec<(u64, String)>,     // id + the forge error
}
```

Aborting at the first failure would leave the backlog half-labelled with no record of
where it stopped. Returning `Ok(())` would claim 59 successes for 58.

### Resolving the labels

Computed from `Config`. Both directions remove more than they add, so the result is
unambiguous to the engine *and* to a human reading the labels:

- **Ready:** `status_labels.transition(Status::Ready)` — the exact transition `release()`
  performs — **plus every skip label**. Without that last part, marking a parked issue
  ready would leave the skip label on, `next_ready_issue` would still refuse it, and the
  board would still show it as NeedsHuman. The button would appear to do nothing.
- **Park:** add `select.skip_labels.first()`, remove the ready label. The engine would skip
  the issue either way (a skip label beats the ready label in both `next_ready_issue` and
  `classify`), but leaving both on shows a human a contradiction. An empty `skip_labels`
  makes park unavailable; `triage_labels` returns a clear error rather than silently
  applying nothing.

## Step 3: the orchestrator proposes the triage

### Generalizing the proposal

`TurnOutcome.proposal` is `Option<GateProposal>`, with one artifact path and one prompt
postamble. It becomes a tagged union:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Proposal {
    Gate(GateProposal),
    Triage(TriageProposal),
}

pub struct TriageProposal {
    pub ready: Vec<u64>,
    pub needs_human: Vec<u64>,
    pub rationale: String,
}
```

`TriageProposal`'s two vectors map exactly onto the two apply actions, so the card can
offer them separately: a user who agrees with the ready list but not the park list applies
one and dismisses the other.

**Untagged gate proposals must still parse.** `session_id` is persisted, so a resumed
claude session can carry the *old* instruction in its context and write the old untagged
`{"commands":[...]}` shape. `read_proposal` therefore tries the tagged union first and
falls back to a bare `GateProposal`. Without that fallback, an in-flight conversation
would silently stop producing gate proposals after this upgrade, and the failure mode
(`read_proposal` returns `None`) is invisible.

### The postamble

`gate_instruction` becomes `proposal_instruction`, describing both shapes against the same
path. The triage half tells the agent to read the backlog from the forge CLI, to split it
into ready / needs-human by whether an issue is specced enough for an agent to act on
alone, and to leave genuinely ambiguous issues out of both lists rather than guessing.

Both proposals share one artifact path. Turns are single-flight and the path is cleared at
the start of each turn, so at most one proposal exists per turn; a turn that would produce
both is a conversation that should have been two turns.

## Frontend

### Pure logic (`$lib/board.ts`, vitest-covered per the existing convention)

- `triageGap(board)` -> `{ total, ready, untriaged, needsHuman }`. The headline number:
  "59 issues, 0 ready, 59 untriaged".
- `boardColumns` gains `needs_human`, hidden when empty, ordered
  `untriaged, needs_human, ready, in_progress, done`.
- Selection is a `Set<number>` with pure `toggle` / `selectAll` / `clear` helpers, so the
  pane holds no selection logic.
- `laneChips` includes needs-human chips with their own class, for the same reason step 1
  included untriaged: dropping them would trade one lie for another.

### Board view

Every column header gains a count. The two selectable columns (Untriaged and Needs human,
mirroring `is_triageable`) also gain a select-all toggle, and their cards gain a checkbox.
A card is currently a single `<button>`, so a checkbox cannot be nested inside it (invalid
HTML, and it would swallow the click). A triageable card becomes a row: a real
`<input type="checkbox">` with a label, next to the existing title button.

Selection spans both columns, so one apply can mix newly-triaged and previously-parked
issues. `pruneSelection` returns the *same set instance* when nothing changed, which is
what lets the pane call it from an `$effect` that also assigns to the selection without
looping on its own write.

A footer bar appears while the selection is non-empty: `Mark N ready` / `Park N`. After
apply, the board refreshes and the outcome is summarised, including anything skipped or
failed.

### Orchestrator pane

`GateProposal` becomes a discriminated union in `orchestrator.ts`. The triage card lists
the counts and the issue numbers, with Apply on each of the two lists.

## Testing

**Rust, hermetic:**

- `classify` for each ordering case, including ready+needs-human (must be NeedsHuman) and
  done+needs-human (must be Done), and a multi-entry `skip_labels`.
- `assemble_board` routes a needs-human issue into `needs_human`, not `untriaged` or
  `ready`.
- `FakeForge::edit_labels` add/remove, including removing an absent label.
- `apply_triage`'s eligibility filter and partial-failure accounting. This needs a
  `FakeForge` that can be told to fail a specific issue, so the `failed` vector is
  asserted rather than assumed.
- A park/un-park round trip asserted through `assemble_board`, so the label sets are
  checked by what the board actually shows rather than by their own spelling. This is the
  test that catches a "Mark ready" which leaves a skip label on.
- `read_proposal`: tagged gate, tagged triage, **untagged legacy gate**, and malformed.

**Frontend (vitest):** `triageGap` counts, column order and hide-when-empty for both
optional columns, selection helpers, and the proposal union reducer.

**Manual:** the one thing the hermetic tests cannot cover is that a real `gh issue edit`
applies the labels a real backlog needs. Called out here so it is not mistaken for
covered.

## Correction: the corpus, not just the labels (found in review)

The design above reasons carefully about *which labels* triage writes and never asks *which
issues* it is handed. That blind spot produced a bug of exactly the shape this feature
exists to kill.

`assemble_board` and `apply_triage` both read `list_issues`, and every adapter fetches it
with `state=all` (the Done column would otherwise be empty, because `record` writes
`status:done` but never closes an issue). `domain::Issue` carried no state, so a closed
issue with no status label — which is what a `Closes #N` auto-merge leaves behind —
classified as **Untriaged**. It inflated the gap banner, sat under "select all", and one
click wrote `status:ready` to it. The engine selects with `state=open` and would pick up
none of them. That is the "board says ready, engine does nothing" lie from issue #16,
recreated as durable forge state rather than a display glitch. On this repo it was 8 issues.

The fix is `IssueState` on `Issue`, parsed per adapter, and a first arm in `classify`:
closed is Done, whatever it is labelled. That also repairs the write path for free, since
`is_triageable(Done)` is already false — the eligibility re-read becomes a real guard
instead of a second opinion computed from the same missing information.

Two things follow from it:

- **Closed outranks every status label**, including in-progress. An issue closed while a
  runner held it is finished, and offering it as live work would be the same lie again.
- **`list_issues` paginates.** With `state=all`, a single 100-issue page is spent on closed
  issues first, so the older *open* backlog was both invisible on the board and unreachable
  by triage. Gitea and GitLab now loop pages; GitHub raises `--limit`, since `gh` paginates
  internally. The page loop tests the **raw** element count, not the parsed one:
  `parse_issue_list` drops pull requests, so a full page routinely parses short and testing
  the parsed length would end pagination early and silently truncate the backlog.

## Correction: the approval has to be informed (found in security review)

The first version of the triage card rendered bare issue numbers and a count: "Mark 5
ready" over `#4 #9 #12 #17 #23`. That is not an approval a user can reason about, and it
sits at the one place in this tool where the safety model actually rests.

The chain: `proposal_instruction` tells the orchestrator to read the open issues from the
forge CLI, so on a public repo an issue body written by an anonymous stranger enters the
agent's context. Text in that body can steer the agent into listing its own issue under
`ready`. The user approves five integers. The next drain hands the issue to an implementer
running with permissions skipped, in the real checkout. `require_label` is the only control
between "a stranger wrote this" and "an agent acts on it", and this card is the mechanism
that flips it.

So the card now resolves every proposed id against the forge (`preview_triage`) and shows
the real title and current board status, and **Apply stays disabled until that resolve
succeeds**: a proposal that cannot be checked is precisely the one not to approve. Ids the
forge does not return are shown as "(no such issue)" rather than quietly dropped, and a
proposed Ready for an issue currently `NeedsHuman` is marked "un-parks", because reversing
a human's deliberate decision should never be silent. The resolve is deliberately a fresh
read rather than data carried in the proposal, since the proposal may be minutes old.

Ineligible entries stay visible and greyed rather than being filtered out, and Apply acts
only on the eligible ones, so the count on the button matches what will actually change.

## Correction: the proposal artifact was in world-writable /tmp

`std::env::temp_dir()` is `/tmp` on Linux, and a process id is readable from `/proc`. Any
local user could pre-create `tutti-gate-<pid>.json`; the sticky bit would then block our
unlink, whose error was discarded with `let _ =`; and `read_proposal` would return the
attacker's JSON. A gate proposal becomes `[gate].commands`, which the engine runs through
`sh -c`.

Two changes. The artifact moved to `<app data dir>/proposals/`, which is per-user and not
world-writable (clamped to 0700 on unix for a permissive parent). And clearing a stale
artifact is now fatal on any error other than `NotFound`: if the file exists and cannot be
removed, something else owns it, and whatever is read back is not the agent's proposal.

## Out of scope

- Bulk triage by milestone or by search query. Selection is explicit for now.
- Undo. The forge's own issue history is the audit trail; a triage decision is one label
  edit and is reversible by the opposite action.
- Teaching the engine anything new. This slice writes labels the engine already reads.
