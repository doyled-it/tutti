<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->

# Board bucketing: untriaged issues are not Ready (#16, step 1)

## Problem

`assemble_board` in `crates/tutti-app-core/src/lib.rs` buckets any issue with no
recognized status label into the `ready` column:

```rust
Status::Other => ready.push(c), // untriaged shows under Ready
```

The engine, however, selects issues on `require_label` (`status:ready`). An untriaged
issue carries none of the status labels, so a Run picks up **zero** of them and stops
immediately. A repo like oxidra therefore shows ~59 issues under READY while a Run would
do nothing. The board says one thing and the engine does another, and it is the worst
version of that: it looks ready to go.

## Scope

Step 1 of issue #16 only: make untriaged issues visibly their own thing on the board,
not Ready. Explicitly **out of scope** (deferred, they stay in #16 for a later pass):

- The "Convert this repo" bulk-label flow (show the gap, select issues, apply
  `status:ready`).
- Orchestrator-assisted triage that proposes which issues are ready (belongs partly to
  #15).

## Decisions

- **Separate UNTRIAGED column**, rendered as the leftmost column, matching the existing
  Ready / In progress / Done columns. This is the cleanest mapping to engine reality: a
  Run picks up zero untriaged issues, and the board shows them as a distinct, upstream
  bucket rather than smuggling them into Ready.
- **Hidden when empty.** A fully triaged repo (no untriaged issues) does not render the
  UNTRIAGED column at all, so the column only appears when there is a real gap to act on.
  The other three columns keep their existing always-show behavior with the "Nothing
  here" empty state.
- **Rename `Status::Other` to `Status::Untriaged`.** The point of this fix is legibility.
  A card whose status serializes as `"other"` sitting under a column labeled "Untriaged"
  is a smaller version of the same lie. The rename makes the data honest end to end. It
  is safe: no frontend code branches on `"other"` or `"ready"` today (only
  `"in_progress"` and `"done"`).

## Changes

### `tutti-app-core` (`crates/tutti-app-core/src/lib.rs`)

1. Rename the `Status::Other` variant to `Status::Untriaged`. `classify`'s fallback arm
   (an issue matching none of done / in_progress / ready) returns `Status::Untriaged`.
   The serde value becomes `"untriaged"`.
2. Add an `untriaged: Vec<IssueCard>` field to `Board`.
3. In `assemble_board`, route `Status::Untriaged` cards into `untriaged` instead of
   `ready`. The `ready` column now contains only genuine `status:ready` issues. The
   misleading comment is removed.

### Frontend types (`tutti-app/src/lib/ipc.ts`)

4. `Status` union: replace `"other"` with `"untriaged"`.
5. `Board` interface: add `untriaged: IssueCard[]`.

### Board view (`tutti-app/src/lib/components/BoardView.svelte`)

6. Prepend an `{ key: "untriaged", label: "Untriaged" }` column as the leftmost, but
   render it only when `board.untriaged.length > 0` (hide-when-empty). The Ready /
   In progress / Done columns are unchanged and still always shown.
7. Untriaged cards get a distinct visual treatment (e.g. a dashed border or dimmed
   accent) so they read as "needs a label", not as actionable work.

### Lanes view (`tutti-app/src/lib/components/LanesView.svelte`)

8. Include `board.untriaged` chips with a distinct class (`"u"`). Without this, untriaged
   issues would vanish from the milestone swimlane entirely: today they appear only
   because they are smuggled into `ready`. Dropping them silently would trade one lie
   (they look Ready) for another (they disappear).

## Testing

- **Unit (Rust, hermetic, FakeForge):** add an `assemble_board` case that seeds an issue
  with no status label and asserts it lands in `board.untriaged` and **not** in
  `board.ready`. Existing bucketing tests continue to assert the labeled paths.
- **Frontend (vitest):** the existing `applyEvent` / store tests do not touch bucketing
  and must stay green. The column-visibility rule is a simple length guard; a small
  component-level assertion is optional.

## Non-goals recap

No new Tauri command, no forge write path, no bulk labeling, no orchestrator work. This
is a display-correctness fix confined to `assemble_board`, the `Board`/`Status` types,
and the two views that render them.
