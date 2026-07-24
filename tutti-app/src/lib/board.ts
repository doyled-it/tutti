// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure helpers for the board and lanes views: which columns to show and in what order,
// and the ordered, style-tagged lane chips. No Svelte or Tauri imports so all of it is
// unit-testable.
import type { Board, IssueCard } from "./ipc";

/** A board bucket that is rendered as its own column. */
export type ColumnKey = keyof Pick<Board, "untriaged" | "ready" | "in_progress" | "done">;

/** A lane chip: one card plus the short style class its status maps to. */
export type LaneChip = { card: IssueCard; cls: string };

/**
 * The board columns in display order. Untriaged leads (it is upstream of Ready) but only
 * appears when the bucket is non-empty, so a fully triaged repo shows just the three
 * always-present columns. Ready / In progress / Done are always shown.
 */
export function boardColumns(board: Board): { key: ColumnKey; label: string }[] {
  const all: { key: ColumnKey; label: string }[] = [
    { key: "untriaged", label: "Untriaged" },
    { key: "ready", label: "Ready" },
    { key: "in_progress", label: "In progress" },
    { key: "done", label: "Done" },
  ];
  return all.filter((c) => c.key !== "untriaged" || board.untriaged.length > 0);
}

/**
 * The lane chips in display order (untriaged first, then ready / in progress / done),
 * each tagged with the one-letter class the lanes view styles. Untriaged is included so
 * those issues stay visible in the swimlane rather than vanishing.
 */
export function laneChips(board: Board): LaneChip[] {
  return [
    ...board.untriaged.map((c) => ({ card: c, cls: "u" })),
    ...board.ready.map((c) => ({ card: c, cls: "r" })),
    ...board.in_progress.map((c) => ({ card: c, cls: "i" })),
    ...board.done.map((c) => ({ card: c, cls: "d" })),
  ];
}
