// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  appendDelta,
  appendProposal,
  appendTool,
  dropTrailingEmptyAssistant,
  removeProposalAt,
  proposalSummary,
  startAssistant,
  type ChatMessage,
  type Proposal,
} from "./orchestrator";

describe("orchestrator reducer", () => {
  it("accumulates deltas into the open assistant bubble", () => {
    let msgs: ChatMessage[] = [{ role: "user", text: "hi", kind: "text" }];
    msgs = startAssistant(msgs);
    msgs = appendDelta(msgs, "Hello ");
    msgs = appendDelta(msgs, "world");
    expect(msgs).toHaveLength(2);
    expect(msgs[1]).toEqual({ role: "assistant", text: "Hello world", kind: "text" });
  });

  it("inserts a tool aside without disturbing the open bubble", () => {
    let msgs: ChatMessage[] = [];
    msgs = startAssistant(msgs);
    msgs = appendTool(msgs, "Bash");
    msgs = appendDelta(msgs, "done");
    // tool aside is its own message; the trailing text bubble carries the reply.
    expect(msgs.some((m) => m.kind === "tool" && m.text === "Bash")).toBe(true);
    expect(msgs[msgs.length - 1]).toEqual({ role: "assistant", text: "done", kind: "text" });
  });

  it("drops a trailing empty assistant bubble left after a tool call", () => {
    let msgs: ChatMessage[] = [{ role: "user", text: "hi", kind: "text" }];
    msgs = appendTool(msgs, "Bash"); // leaves an empty text bubble after the tool aside
    msgs = dropTrailingEmptyAssistant(msgs);
    expect(msgs.map((m) => m.kind)).toEqual(["text", "tool"]);
    expect(msgs[msgs.length - 1]).toEqual({ role: "assistant", text: "Bash", kind: "tool" });
  });

  it("leaves a non-empty trailing bubble untouched", () => {
    const msgs: ChatMessage[] = [{ role: "assistant", text: "done", kind: "text" }];
    expect(dropTrailingEmptyAssistant(msgs)).toEqual(msgs);
  });

  it("appends a proposal card and removes it by index", () => {
    let msgs: ChatMessage[] = [{ role: "user", text: "gate?", kind: "text" }];
    const proposal: Proposal = {
      kind: "gate",
      commands: ["cargo test"],
      working_dir: "",
      rationale: "rust",
    };
    msgs = appendProposal(msgs, proposal);
    const idx = msgs.length - 1;
    expect(msgs[idx].kind).toBe("proposal");
    expect(msgs[idx].proposal).toEqual(proposal);
    msgs = removeProposalAt(msgs, idx);
    expect(msgs.some((m) => m.kind === "proposal")).toBe(false);
  });

  it("replaces a prior proposal card and drops a trailing empty bubble", () => {
    let msgs: ChatMessage[] = [{ role: "user", text: "hi", kind: "text" }];
    msgs = appendTool(msgs, "Bash"); // leaves a trailing empty assistant text bubble
    msgs = appendProposal(msgs, {
      kind: "gate",
      commands: ["a"],
      working_dir: "",
      rationale: "",
    });
    // The empty bubble is gone and exactly one card shows.
    expect(msgs.some((m) => m.kind === "text" && m.text === "")).toBe(false);
    expect(msgs.filter((m) => m.kind === "proposal")).toHaveLength(1);
    // A second proposal replaces the first rather than stacking.
    const second: Proposal = { kind: "gate", commands: ["b"], working_dir: "", rationale: "" };
    msgs = appendProposal(msgs, second);
    expect(msgs.filter((m) => m.kind === "proposal")).toHaveLength(1);
    expect(msgs[msgs.length - 1].proposal).toEqual(second);
  });

  it("carries a triage proposal on the card", () => {
    const triage: Proposal = {
      kind: "triage",
      ready: [4, 9],
      needs_human: [12],
      rationale: "4 and 9 are specced",
    };
    const msgs = appendProposal([], triage);
    expect(msgs[0].kind).toBe("proposal");
    expect(msgs[0].proposal).toEqual(triage);
  });

  it("replaces a gate card with a triage card rather than stacking them", () => {
    // One artifact path means one live proposal; two cards would let the user apply a
    // proposal the agent has already moved on from.
    let msgs = appendProposal([], {
      kind: "gate",
      commands: ["a"],
      working_dir: "",
      rationale: "",
    });
    msgs = appendProposal(msgs, { kind: "triage", ready: [1], needs_human: [], rationale: "" });
    expect(msgs.filter((m) => m.kind === "proposal")).toHaveLength(1);
    expect(msgs[msgs.length - 1].proposal?.kind).toBe("triage");
  });
});

describe("proposalSummary", () => {
  it("joins gate commands", () => {
    expect(
      proposalSummary({ kind: "gate", commands: ["a", "b"], working_dir: "", rationale: "" }),
    ).toBe("a && b");
  });

  it("counts both triage lists", () => {
    expect(
      proposalSummary({ kind: "triage", ready: [1, 2], needs_human: [3], rationale: "" }),
    ).toBe("Triage: 2 ready, 1 needs human");
  });

  it("omits an empty triage list", () => {
    expect(proposalSummary({ kind: "triage", ready: [1], needs_human: [], rationale: "" })).toBe(
      "Triage: 1 ready",
    );
  });

  it("describes a triage with nothing in it", () => {
    expect(proposalSummary({ kind: "triage", ready: [], needs_human: [], rationale: "" })).toBe(
      "Triage: nothing to apply",
    );
  });
});
