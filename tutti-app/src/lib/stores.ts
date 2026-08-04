// SPDX-License-Identifier: AGPL-3.0-or-later
// Shared app state: the loaded project, the current board, and the live run status. The
// applyEvent reducer is the tested, pure core of the live-update behavior.

import { writable } from "svelte/store";
import type { Board, EngineEvent, GateStatus, IssueCard, ProjectEntry } from "./ipc";
import { emptySubsessions, type SubsessionState } from "./subsessions";

/** The full saved project list, restored on launch and kept in sync with the backend. */
export const projects = writable<ProjectEntry[]>([]);

/** The `dir` of the active project, or null when nothing is active. */
export const activeDir = writable<string | null>(null);

export const board = writable<Board | null>(null);

/** The active project's gate status (null before a project loads). Drives the no-op badge. */
export const gateStatus = writable<GateStatus | null>(null);

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

/** Which sidebar section is active: the project board, orchestrator chat, or subsessions. */
export const section = writable<"board" | "orchestrator" | "subsessions">("board");

/**
 * True while an orchestrator chat turn is in flight. The sidebar blocks project switch/add/
 * remove while it is set (the same posture as an active engine run), so a project cannot be
 * swapped out from under a running turn. The backend enforces its own single-flight guard;
 * this store only gates the UI.
 */
export const orchestratorBusy = writable(false);

/**
 * Live subsession state for the current run (in-memory, never persisted). Populated by the
 * subsession://event stream via applySubsession, and cleared when a new run starts.
 */
export const subsessions = writable<SubsessionState>(emptySubsessions());

/** Reset the subsessions view. Called when a run starts (the "new run" boundary). */
export function clearSubsessions(): void {
  subsessions.set(emptySubsessions());
}

/** Pure reducer: apply one engine event to a board + run-status snapshot. Exported for tests. */
export function applyEvent(
  b: Board | null,
  r: RunUi,
  ev: EngineEvent,
): { board: Board | null; run: RunUi } {
  if (!b) return { board: b, run: r };
  const move = (id: number, to: "ready" | "in_progress" | "done") => {
    // Search and strip every bucket, including untriaged: if the board is stale relative
    // to the forge (an issue was relabeled status:ready and claimed before the next full
    // refresh), a claim event can target a card still sitting in untriaged. Missing it
    // would leave a phantom copy in untriaged as well as the moved-to column.
    const all = [...b.ready, ...b.in_progress, ...b.done, ...b.untriaged];
    const found = all.find((c) => c.id === id);
    const strip = (xs: IssueCard[]) => xs.filter((c) => c.id !== id);
    const nb: Board = {
      ...b,
      ready: strip(b.ready),
      in_progress: strip(b.in_progress),
      done: strip(b.done),
      untriaged: strip(b.untriaged),
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
