// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  appendDelta,
  appendTool,
  dropTrailingEmptyAssistant,
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
});
