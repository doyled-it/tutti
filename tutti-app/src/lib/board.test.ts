// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import { boardColumns, laneChips } from "./board";
import type { Board, IssueCard } from "./ipc";

function card(id: number, status: IssueCard["status"]): IssueCard {
  return { id, title: `#${id}`, status, milestone: null };
}

function board(over: Partial<Board>): Board {
  return {
    milestones: [],
    selected_milestone: null,
    ready: [],
    in_progress: [],
    done: [],
    untriaged: [],
    ...over,
  };
}

describe("boardColumns", () => {
  it("omits the untriaged column when the bucket is empty", () => {
    const cols = boardColumns(board({ ready: [card(1, "ready")] }));
    expect(cols.map((c) => c.key)).toEqual(["ready", "in_progress", "done"]);
  });

  it("prepends the untriaged column when the bucket is non-empty", () => {
    const cols = boardColumns(board({ untriaged: [card(1, "untriaged")] }));
    expect(cols.map((c) => c.key)).toEqual(["untriaged", "ready", "in_progress", "done"]);
    expect(cols[0].label).toBe("Untriaged");
  });

  it("always shows ready / in progress / done regardless of contents", () => {
    const cols = boardColumns(board({}));
    expect(cols.map((c) => c.key)).toEqual(["ready", "in_progress", "done"]);
  });
});

describe("laneChips", () => {
  it("orders untriaged first, then ready, in progress, done", () => {
    const chips = laneChips(
      board({
        ready: [card(2, "ready")],
        in_progress: [card(3, "in_progress")],
        done: [card(4, "done")],
        untriaged: [card(1, "untriaged")],
      }),
    );
    expect(chips.map((c) => c.card.id)).toEqual([1, 2, 3, 4]);
    expect(chips.map((c) => c.cls)).toEqual(["u", "r", "i", "d"]);
  });

  it("still includes untriaged issues so they do not vanish from the swimlane", () => {
    const chips = laneChips(board({ untriaged: [card(1, "untriaged"), card(2, "untriaged")] }));
    expect(chips.map((c) => c.card.id)).toEqual([1, 2]);
    expect(chips.every((c) => c.cls === "u")).toBe(true);
  });
});
