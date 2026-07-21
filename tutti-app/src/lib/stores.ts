// SPDX-License-Identifier: AGPL-3.0-or-later
// Shared app state: the loaded project, the current board, and the live run status. The
// applyEvent reducer is the tested, pure core of the live-update behavior.

import { writable } from "svelte/store";
import type { Board, EngineEvent, IssueCard, ProjectEntry } from "./ipc";

/** The full saved project list, restored on launch and kept in sync with the backend. */
export const projects = writable<ProjectEntry[]>([]);

/** The `dir` of the active project, or null when nothing is active. */
export const activeDir = writable<string | null>(null);

export const board = writable<Board | null>(null);

export type RunUi = {
  state: "idle" | "running" | "pausing";
  current?: string;
  shipped: number;
};

export const runStatus = writable<RunUi>({ state: "idle", shipped: 0 });

/** The id of the issue currently shown in the drawer, or null when it is closed. */
export const selectedIssueId = writable<number | null>(null);

/** Which main-pane view is active: the Kanban board or the milestone lanes. */
export const view = writable<"board" | "lanes">("board");

/** Pure reducer: apply one engine event to a board + run-status snapshot. Exported for tests. */
export function applyEvent(
  b: Board | null,
  r: RunUi,
  ev: EngineEvent,
): { board: Board | null; run: RunUi } {
  if (!b) return { board: b, run: r };
  const move = (id: number, to: "ready" | "in_progress" | "done") => {
    const all = [...b.ready, ...b.in_progress, ...b.done];
    const found = all.find((c) => c.id === id);
    const strip = (xs: IssueCard[]) => xs.filter((c) => c.id !== id);
    const nb: Board = {
      ...b,
      ready: strip(b.ready),
      in_progress: strip(b.in_progress),
      done: strip(b.done),
    };
    if (found) {
      const card = { ...found, status: to } as IssueCard;
      (nb[to] as IssueCard[]) = [...nb[to], card];
    }
    return nb;
  };
  switch (ev.kind) {
    case "drain_started":
      return { board: b, run: { ...r, state: "running" } };
    case "issue_claimed":
      return {
        board: move(ev.id, "in_progress"),
        run: { ...r, current: `#${ev.id} ${ev.title}` },
      };
    case "issue_shipped":
      return {
        board: move(ev.id, "done"),
        run: { ...r, shipped: r.shipped + 1, current: undefined },
      };
    case "issue_released":
      return { board: move(ev.id, "ready"), run: { ...r, current: undefined } };
    case "drain_complete":
      // A per-pass completion, not the end of the run: the continuous loop drains again
      // until nothing is ready, so this must NOT flip the UI to idle (that would flicker
      // between passes). The run's true end arrives via engine://run-ended.
      return { board: b, run: r };
  }
}
