// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  emptySubsessions,
  applySubsession,
  roleLabel,
  selectSubsession,
  MAX_SUBSESSIONS,
} from "./subsessions";

describe("applySubsession", () => {
  it("opens a subsession on started and selects it", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "do it" });
    expect(s.list).toHaveLength(1);
    expect(s.list[0].key).toBe("42:implementer");
    expect(s.list[0].status).toBe("running");
    expect(s.list[0].title).toBe("do it");
    expect(s.selected).toBe("42:implementer");
  });

  it("routes delta and tool text into the matching subsession", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    s = applySubsession(s, { kind: "delta", issue: 42, role: "implementer", text: "hello " });
    s = applySubsession(s, { kind: "delta", issue: 42, role: "implementer", text: "world" });
    s = applySubsession(s, { kind: "tool", issue: 42, role: "implementer", name: "Edit" });
    const msgs = s.list[0].messages;
    // first bubble accumulates the deltas, then a tool aside, then a reopened empty bubble
    expect(msgs).toHaveLength(3);
    expect(msgs[0]).toMatchObject({ role: "assistant", kind: "text", text: "hello world" });
    expect(msgs.some((m) => m.kind === "tool" && m.text === "Edit")).toBe(true);
    expect(msgs[2]).toMatchObject({ role: "assistant", kind: "text", text: "" });
  });

  it("marks completed with status + summary and drops a trailing empty bubble", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "reviewer", title: "t" });
    s = applySubsession(s, { kind: "tool", issue: 42, role: "reviewer", name: "Read" });
    s = applySubsession(s, {
      kind: "completed",
      issue: 42,
      role: "reviewer",
      summary: "request changes (2 findings)",
      ok: false,
    });
    const sub = s.list[0];
    expect(sub.status).toBe("error");
    expect(sub.summary).toBe("request changes (2 findings)");
    // the empty text bubble reopened after the tool aside is dropped on completion
    expect(sub.messages[sub.messages.length - 1].kind).not.toBe("text");
  });

  it("marks completed with status done on the ok:true happy path", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    s = applySubsession(s, {
      kind: "completed",
      issue: 42,
      role: "implementer",
      summary: "ready to ship",
      ok: true,
    });
    const sub = s.list[0];
    expect(sub.status).toBe("done");
    expect(sub.summary).toBe("ready to ship");
  });

  it("does not mutate the prior state when applying an event", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    const priorList = s.list;
    Object.freeze(s);
    Object.freeze(s.list);
    const next = applySubsession(s, { kind: "delta", issue: 42, role: "implementer", text: "x" });
    expect(next).not.toBe(s);
    expect(next.list).not.toBe(priorList);
    expect(s.list).toBe(priorList);
    expect(s.list[0].messages[0].text).toBe("");
  });

  it("advances selection to the newest started (follows the live edge)", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    s = applySubsession(s, { kind: "started", issue: 42, role: "reviewer", title: "t" });
    expect(s.selected).toBe("42:reviewer");
    expect(s.list.map((x) => x.key)).toEqual(["42:implementer", "42:reviewer"]);
  });

  it("resets an existing key's transcript on a re-started turn", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    s = applySubsession(s, { kind: "delta", issue: 42, role: "implementer", text: "old" });
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    expect(s.list).toHaveLength(1);
    expect(s.list[0].messages.every((m) => m.text === "")).toBe(true);
    expect(s.list[0].status).toBe("running");
  });

  it("ignores a delta for an unknown key", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "delta", issue: 99, role: "planner", text: "x" });
    expect(s.list).toHaveLength(0);
  });

  it("labels the planner without an issue number", () => {
    expect(roleLabel({ issue: 0, role: "planner" })).toBe("Planner");
    expect(roleLabel({ issue: 42, role: "fix_applier" })).toBe("#42 Fix applier");
  });

  it("selects an existing key", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 42, role: "implementer", title: "t" });
    s = applySubsession(s, { kind: "started", issue: 42, role: "reviewer", title: "t" });
    s = selectSubsession(s, "42:implementer");
    expect(s.selected).toBe("42:implementer");
  });

  it("evicts the oldest completed subsession once past the cap", () => {
    let s = emptySubsessions();
    const total = MAX_SUBSESSIONS + 3;
    for (let issue = 1; issue <= total; issue++) {
      s = applySubsession(s, { kind: "started", issue, role: "implementer", title: "t" });
      s = applySubsession(s, {
        kind: "completed",
        issue,
        role: "implementer",
        summary: "ready to ship",
        ok: true,
      });
    }
    expect(s.list).toHaveLength(MAX_SUBSESSIONS);
    expect(s.list.some((x) => x.key === "1:implementer")).toBe(false);
    expect(s.list.some((x) => x.key === `${total}:implementer`)).toBe(true);
  });

  it("never evicts a running or the selected subsession", () => {
    let s = emptySubsessions();
    s = applySubsession(s, { kind: "started", issue: 1, role: "implementer", title: "t" });
    const total = MAX_SUBSESSIONS + 3;
    for (let issue = 2; issue <= total; issue++) {
      s = applySubsession(s, { kind: "started", issue, role: "implementer", title: "t" });
      s = applySubsession(s, {
        kind: "completed",
        issue,
        role: "implementer",
        summary: "ready to ship",
        ok: true,
      });
    }
    expect(s.list.some((x) => x.key === "1:implementer")).toBe(true);
    expect(s.list.find((x) => x.key === "1:implementer")?.status).toBe("running");
  });
});
