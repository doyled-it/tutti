// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  appendDelta,
  appendProposal,
  appendTool,
  dropTrailingEmptyAssistant,
  removeProposalAt,
  startAssistant,
  type ChatMessage,
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
    const proposal = { commands: ["cargo test"], working_dir: "", rationale: "rust" };
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
    msgs = appendProposal(msgs, { commands: ["a"], working_dir: "", rationale: "" });
    // The empty bubble is gone and exactly one card shows.
    expect(msgs.some((m) => m.kind === "text" && m.text === "")).toBe(false);
    expect(msgs.filter((m) => m.kind === "proposal")).toHaveLength(1);
    // A second proposal replaces the first rather than stacking.
    const second = { commands: ["b"], working_dir: "", rationale: "" };
    msgs = appendProposal(msgs, second);
    expect(msgs.filter((m) => m.kind === "proposal")).toHaveLength(1);
    expect(msgs[msgs.length - 1].proposal).toEqual(second);
  });
});
