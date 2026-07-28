// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure transcript reducer for the orchestrator chat. Mirrors the Rust TranscriptMessage
// (crates/tutti-app-core/src/lib.rs). Kept pure and vitest-covered per the board.ts /
// browse.ts convention; the Svelte pane holds only wiring.

export type MessageKind = "text" | "tool" | "proposal";

export interface GateProposal {
  commands: string[];
  working_dir: string;
  rationale: string;
}

export interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  kind: MessageKind;
  // Present only when kind === "proposal".
  proposal?: GateProposal;
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

// Drop a trailing empty assistant text bubble. `appendTool` reopens a bubble for the text
// that usually follows a tool call; a turn that ends on a tool (or produces no text) leaves
// that bubble empty. Called on turn completion so the live view matches the persisted
// transcript, which never stores an empty assistant message.
export function dropTrailingEmptyAssistant(msgs: ChatMessage[]): ChatMessage[] {
  const last = msgs[msgs.length - 1];
  if (last && last.role === "assistant" && last.kind === "text" && last.text === "") {
    return msgs.slice(0, -1);
  }
  return msgs;
}

// Append a gate-proposal card. Live-only (never persisted): the agent proposes, the user
// applies or dismisses. `text` carries a short human summary for accessibility.
export function appendProposal(msgs: ChatMessage[], proposal: GateProposal): ChatMessage[] {
  return [
    ...msgs,
    { role: "assistant", text: proposal.commands.join(" && "), kind: "proposal", proposal },
  ];
}

// Remove the proposal card at `index` (on apply or dismiss). Out-of-range is a no-op.
export function removeProposalAt(msgs: ChatMessage[], index: number): ChatMessage[] {
  if (index < 0 || index >= msgs.length) return msgs;
  return [...msgs.slice(0, index), ...msgs.slice(index + 1)];
}
