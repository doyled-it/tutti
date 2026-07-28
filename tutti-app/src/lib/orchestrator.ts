// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure transcript reducer for the orchestrator chat. Mirrors the Rust TranscriptMessage
// (crates/tutti-app-core/src/lib.rs). Kept pure and vitest-covered per the board.ts /
// browse.ts convention; the Svelte pane holds only wiring.

export type MessageKind = "text" | "tool";

export interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  kind: MessageKind;
}

// Open a fresh assistant text bubble that deltas will accumulate into.
export function startAssistant(msgs: ChatMessage[]): ChatMessage[] {
  return [...msgs, { role: "assistant", text: "", kind: "text" }];
}

// Append streamed text to the last message when it is an open assistant text bubble;
// otherwise open one first (defensive: a delta arriving with no bubble still shows).
export function appendDelta(msgs: ChatMessage[], text: string): ChatMessage[] {
  const last = msgs[msgs.length - 1];
  if (last && last.role === "assistant" && last.kind === "text") {
    const updated = { ...last, text: last.text + text };
    return [...msgs.slice(0, -1), updated];
  }
  return [...msgs, { role: "assistant", text, kind: "text" }];
}

// Insert a tool-use aside, then reopen a text bubble so subsequent deltas land after it.
export function appendTool(msgs: ChatMessage[], name: string): ChatMessage[] {
  return startAssistant([...msgs, { role: "assistant", text: name, kind: "tool" }]);
}
