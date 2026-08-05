// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure helpers for the board and lanes views: which columns to show and in what order,
// the ordered, style-tagged lane chips, and the triage selection/summary logic. No Svelte
// or Tauri imports so all of it is unit-testable.
import type { Board, IssueCard, TriageOutcome, TriageTarget } from "./ipc";

/** A board bucket that is rendered as its own column. */
export type ColumnKey = keyof Pick<
  Board,
  "untriaged" | "needs_human" | "ready" | "in_progress" | "done"
>;

/**
 * Every bucket the board splits issues into. Derived from rather than duplicated by the
 * places that need to walk them all: `stores.ts` previously spelled the list out twice and
 * carried a comment warning that a new bucket must be added there too, which is a drift
 * hazard documented instead of removed.
 */
export const BOARD_BUCKETS: ColumnKey[] = [
  "untriaged",
  "needs_human",
  "ready",
  "in_progress",
  "done",
];

/** A lane chip: one card plus the short style class its status maps to. */
export type LaneChip = { card: IssueCard; cls: string };

/** The triage gap: how much of the backlog the engine can actually act on. */
export type TriageGap = { total: number; untriaged: number; ready: number; needsHuman: number };

/**
 * The board columns in display order. Untriaged and Needs human both lead (they are
 * upstream of Ready) but each appears only when its bucket is non-empty, so a fully
 * triaged repo shows just the three always-present columns. Ready / In progress / Done
 * are always shown.
 */
export function boardColumns(board: Board): { key: ColumnKey; label: string }[] {
  const optional: ColumnKey[] = ["untriaged", "needs_human"];
  const all: { key: ColumnKey; label: string }[] = [
    { key: "untriaged", label: "Untriaged" },
    { key: "needs_human", label: "Needs human" },
    { key: "ready", label: "Ready" },
    { key: "in_progress", label: "In progress" },
    { key: "done", label: "Done" },
  ];
  return all.filter((c) => !optional.includes(c.key) || board[c.key].length > 0);
}

/**
 * The lane chips in display order (untriaged, needs human, then ready / in progress /
 * done), each tagged with the one-letter class the lanes view styles. Untriaged and
 * needs-human are included so those issues stay visible in the swimlane rather than
 * vanishing.
 */
export function laneChips(board: Board): LaneChip[] {
  return [
    ...board.untriaged.map((c) => ({ card: c, cls: "u" })),
    ...board.needs_human.map((c) => ({ card: c, cls: "h" })),
    ...board.ready.map((c) => ({ card: c, cls: "r" })),
    ...board.in_progress.map((c) => ({ card: c, cls: "i" })),
    ...board.done.map((c) => ({ card: c, cls: "d" })),
  ];
}

/**
 * Count the backlog by what the engine would do with it. The headline a freshly added
 * pre-Tutti repo needs: "59 issues, 0 ready, 59 untriaged".
 */
export function triageGap(board: Board): TriageGap {
  return {
    total: BOARD_BUCKETS.reduce((n, k) => n + board[k].length, 0),
    untriaged: board.untriaged.length,
    ready: board.ready.length,
    needsHuman: board.needs_human.length,
  };
}

/**
 * The columns whose cards can be triaged, mirroring the backend's `is_triageable`. In
 * progress and Done are absent on purpose: the in-progress label IS the claim lock, and
 * shipped work is never re-opened. Needs human is present so parking is not a one-way door.
 */
const SELECTABLE_COLUMNS: ColumnKey[] = ["untriaged", "needs_human"];

export function isSelectable(key: ColumnKey): boolean {
  return SELECTABLE_COLUMNS.includes(key);
}

/** Add or remove `id` from the selection. Returns a new set; never mutates the input. */
export function toggleSelected(selected: Set<number>, id: number): Set<number> {
  const next = new Set(selected);
  if (!next.delete(id)) next.add(id);
  return next;
}

/** True when every card in `key` is selected. Drives the header toggle's label. */
export function columnFullySelected(selected: Set<number>, board: Board, key: ColumnKey): boolean {
  return board[key].length > 0 && board[key].every((c) => selected.has(c.id));
}

/**
 * Select every card in one column, or clear them if they are all already selected. Other
 * columns' selections are left alone, so a selection can span both upstream columns. The
 * branch lives here rather than in the template so it is a unit-tested fact.
 */
export function toggleColumn(selected: Set<number>, board: Board, key: ColumnKey): Set<number> {
  if (columnFullySelected(selected, board, key)) {
    const drop = new Set(board[key].map((c) => c.id));
    return new Set([...selected].filter((id) => !drop.has(id)));
  }
  return new Set([...selected, ...board[key].map((c) => c.id)]);
}

/** Every id currently selectable, used to prune a selection after the board refreshes. */
function selectableIds(board: Board): Set<number> {
  return new Set(SELECTABLE_COLUMNS.flatMap((k) => board[k].map((c) => c.id)));
}

/**
 * Drop selected ids that are no longer selectable. A refreshed board moves just-labelled
 * issues out of the upstream columns; without this the footer keeps counting ghosts.
 * Returns the same set instance when nothing changed, so it is safe to call from an effect.
 */
export function pruneSelection(selected: Set<number>, board: Board): Set<number> {
  const live = selectableIds(board);
  const kept = [...selected].filter((id) => live.has(id));
  return kept.length === selected.size ? selected : new Set(kept);
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * A one-line result for a triage apply. Skipped and failed counts are always surfaced
 * when non-zero: the whole point of the backend returning per-issue accounting is that
 * the UI must not report 59 successes for 58.
 */
export function triageSummary(outcome: TriageOutcome, to: TriageTarget): string {
  const tail = [
    outcome.skipped.length > 0 ? `${outcome.skipped.length} skipped` : null,
    outcome.failed.length > 0 ? `${outcome.failed.length} failed` : null,
  ].filter((s): s is string => s !== null);
  const suffix = tail.length > 0 ? ` (${tail.join(", ")})` : "";

  if (outcome.applied.length === 0) {
    return tail.length > 0 ? `Nothing to apply (${tail.join(", ")}).` : "Nothing to apply.";
  }
  const head =
    to === "ready"
      ? `Marked ${plural(outcome.applied.length, "issue")} ready`
      : `Parked ${plural(outcome.applied.length, "issue")}`;
  return `${head}${suffix}.`;
}
