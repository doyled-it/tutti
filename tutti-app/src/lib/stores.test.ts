// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import { applyEvent, type RunUi } from "./stores";
import type { Board } from "./ipc";

const base: Board = {
  milestones: [{ id: 1, title: "P1", open: true, total: 2, done: 0 }],
  selected_milestone: 1,
  ready: [{ id: 10, title: "a", status: "ready", milestone: "P1" }],
  in_progress: [],
  done: [],
  untriaged: [],
};
const idle: RunUi = { state: "idle", shipped: 0 };

describe("applyEvent", () => {
  it("claim moves a card to in_progress and sets current", () => {
    const { board, run } = applyEvent(base, idle, { kind: "issue_claimed", id: 10, title: "a" });
    expect(board!.in_progress.map((c) => c.id)).toEqual([10]);
    expect(board!.ready).toEqual([]);
    expect(run.current).toBe("#10 a");
  });

  it("ship moves the card to done and bumps the count", () => {
    const claimed = applyEvent(base, idle, { kind: "issue_claimed", id: 10, title: "a" });
    const { board, run } = applyEvent(claimed.board, claimed.run, {
      kind: "issue_shipped",
      id: 10,
    });
    expect(board!.done.map((c) => c.id)).toEqual([10]);
    expect(run.shipped).toBe(1);
  });

  it("drain_complete does NOT flip to idle (it is a per-pass event, not the run end)", () => {
    // The continuous loop drains again until nothing is ready; run-state ends via the
    // separate run-ended signal, so a per-pass drain_complete must leave state running.
    const { run } = applyEvent(
      base,
      { state: "running", shipped: 1 },
      { kind: "drain_complete", shipped: 1 },
    );
    expect(run.state).toBe("running");
  });

  it("release moves the card back to ready and clears current", () => {
    const claimed = applyEvent(base, idle, { kind: "issue_claimed", id: 10, title: "a" });
    const { board, run } = applyEvent(claimed.board, claimed.run, {
      kind: "issue_released",
      id: 10,
    });
    expect(board!.ready.map((c) => c.id)).toEqual([10]);
    expect(board!.in_progress).toEqual([]);
    expect(run.current).toBeUndefined();
  });

  it("an event for an id not on the board does not corrupt state", () => {
    const { board } = applyEvent(base, idle, { kind: "issue_shipped", id: 999 });
    expect(board!.ready.map((c) => c.id)).toEqual([10]);
    expect(board!.done).toEqual([]);
  });
});
