// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  boardColumns,
  laneChips,
  triageGap,
  toggleSelected,
  isSelectable,
  toggleColumn,
  columnFullySelected,
  pruneSelection,
  triageSummary,
} from "./board";
import type { Board, IssueCard, TriageOutcome } from "./ipc";

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
    needs_human: [],
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

  it("omits the needs-human column when the bucket is empty", () => {
    const cols = boardColumns(board({ ready: [card(1, "ready")] }));
    expect(cols.map((c) => c.key)).not.toContain("needs_human");
  });

  it("places needs-human after untriaged and before ready", () => {
    const cols = boardColumns(
      board({ untriaged: [card(1, "untriaged")], needs_human: [card(2, "needs_human")] }),
    );
    expect(cols.map((c) => c.key)).toEqual([
      "untriaged",
      "needs_human",
      "ready",
      "in_progress",
      "done",
    ]);
    expect(cols[1].label).toBe("Needs human");
  });

  it("shows needs-human on its own when nothing is untriaged", () => {
    const cols = boardColumns(board({ needs_human: [card(2, "needs_human")] }));
    expect(cols.map((c) => c.key)).toEqual(["needs_human", "ready", "in_progress", "done"]);
  });
});

describe("triageGap", () => {
  it("counts each bucket and the total", () => {
    const g = triageGap(
      board({
        untriaged: [card(1, "untriaged"), card(2, "untriaged")],
        ready: [card(3, "ready")],
        needs_human: [card(4, "needs_human")],
        in_progress: [card(5, "in_progress")],
        done: [card(6, "done")],
      }),
    );
    expect(g).toEqual({ total: 6, untriaged: 2, ready: 1, needsHuman: 1 });
  });

  it("reports the pre-Tutti repo shape: everything untriaged, nothing ready", () => {
    const many = Array.from({ length: 59 }, (_, i) => card(i + 1, "untriaged"));
    const g = triageGap(board({ untriaged: many }));
    expect(g).toEqual({ total: 59, untriaged: 59, ready: 0, needsHuman: 0 });
  });

  it("is all zeroes for an empty board", () => {
    expect(triageGap(board({}))).toEqual({ total: 0, untriaged: 0, ready: 0, needsHuman: 0 });
  });
});

describe("selection helpers", () => {
  it("toggles an id on and back off", () => {
    const a = toggleSelected(new Set<number>(), 3);
    expect([...a]).toEqual([3]);
    expect([...toggleSelected(a, 3)]).toEqual([]);
  });

  it("does not mutate the input set", () => {
    const before = new Set([1]);
    const after = toggleSelected(before, 2);
    expect([...before]).toEqual([1]);
    expect([...after]).toEqual([1, 2]);
  });

  it("marks only the upstream columns selectable", () => {
    // Mirrors the backend's is_triageable: the in-progress label is the claim lock and
    // shipped work is never re-opened.
    expect(isSelectable("untriaged")).toBe(true);
    expect(isSelectable("needs_human")).toBe(true);
    expect(isSelectable("ready")).toBe(false);
    expect(isSelectable("in_progress")).toBe(false);
    expect(isSelectable("done")).toBe(false);
  });

  it("selects a whole column without disturbing another column's selection", () => {
    const b = board({
      untriaged: [card(1, "untriaged"), card(2, "untriaged")],
      needs_human: [card(3, "needs_human")],
    });
    expect([...toggleColumn(new Set([3]), b, "untriaged")].sort()).toEqual([1, 2, 3]);
  });

  it("toggling a fully selected column clears exactly that column", () => {
    const b = board({
      untriaged: [card(1, "untriaged"), card(2, "untriaged")],
      needs_human: [card(3, "needs_human")],
    });
    expect([...toggleColumn(new Set([1, 2, 3]), b, "untriaged")]).toEqual([3]);
  });

  it("reports whether a column is fully selected", () => {
    const b = board({ untriaged: [card(1, "untriaged"), card(2, "untriaged")] });
    expect(columnFullySelected(new Set([1]), b, "untriaged")).toBe(false);
    expect(columnFullySelected(new Set([1, 2]), b, "untriaged")).toBe(true);
    // An empty column is not "fully selected", or the header would offer to clear nothing.
    expect(columnFullySelected(new Set(), board({}), "untriaged")).toBe(false);
  });

  it("prunes ids that left the selectable columns", () => {
    // Issue 1 was just marked ready, so it is gone from untriaged and no longer selectable.
    const b = board({ untriaged: [card(2, "untriaged")], ready: [card(1, "ready")] });
    expect([...pruneSelection(new Set([1, 2]), b)]).toEqual([2]);
  });

  it("keeps needs-human ids when pruning, so parking is not a one-way door", () => {
    const b = board({ needs_human: [card(1, "needs_human")] });
    expect([...pruneSelection(new Set([1]), b)]).toEqual([1]);
  });

  it("returns the same set instance when nothing needs pruning", () => {
    // Identity matters: the caller runs this from an effect that also writes the selection.
    const b = board({ untriaged: [card(1, "untriaged")] });
    const s = new Set([1]);
    expect(pruneSelection(s, b)).toBe(s);
  });
});

describe("triageSummary", () => {
  function outcome(over: Partial<TriageOutcome>): TriageOutcome {
    return { applied: [], skipped: [], failed: [], ...over };
  }

  it("reports a clean apply", () => {
    expect(triageSummary(outcome({ applied: [1, 2, 3] }), "ready")).toBe("Marked 3 issues ready.");
  });

  it("uses the singular for one issue", () => {
    expect(triageSummary(outcome({ applied: [1] }), "ready")).toBe("Marked 1 issue ready.");
  });

  it("names the park action", () => {
    expect(triageSummary(outcome({ applied: [1, 2] }), "needs_human")).toBe("Parked 2 issues.");
  });

  it("surfaces skipped and failed counts rather than claiming full success", () => {
    const s = triageSummary(
      outcome({ applied: [1], skipped: [2], failed: [{ issue: 3, error: "boom" }] }),
      "ready",
    );
    expect(s).toContain("Marked 1 issue ready");
    expect(s).toContain("1 skipped");
    expect(s).toContain("1 failed");
  });

  it("says so when nothing was applied", () => {
    expect(triageSummary(outcome({ skipped: [1, 2] }), "ready")).toBe(
      "Nothing to apply (2 skipped).",
    );
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

  it("includes needs-human chips, after untriaged, with their own class", () => {
    const chips = laneChips(
      board({
        untriaged: [card(1, "untriaged")],
        needs_human: [card(2, "needs_human")],
        ready: [card(3, "ready")],
      }),
    );
    expect(chips.map((c) => c.card.id)).toEqual([1, 2, 3]);
    expect(chips.map((c) => c.cls)).toEqual(["u", "h", "r"]);
  });
});
