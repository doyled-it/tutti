// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure model and reducers for the Design surface. Mirrors the Rust DTOs in
// src-tauri/src/design.rs. Kept pure and vitest-covered per the orchestrator.ts / board.ts
// convention; the DesignPane component holds only wiring.

// The eight movement ids, serialized snake_case by tutti-design's MovementId.
export type MovementId =
  "constitution" | "frame" | "impact" | "domain" | "decide" | "structure" | "slice" | "decompose";

// The three project shapes, serialized snake_case by tutti-design's ProjectShape.
export type ProjectShape = "small_cli" | "mobile" | "multi_service";

// The result of one command turn, mirroring the Rust `DesignStep` (serde tag = "kind",
// snake_case). `advanced` is a ratified movement with another queued; `complete` is the last.
export type DesignStep =
  | { kind: "question"; question: string }
  | { kind: "ratify"; artifact_section: string }
  | { kind: "advanced"; movement: MovementId; next: MovementId }
  | { kind: "complete"; movement: MovementId };

export interface DesignArtifact {
  movement: MovementId;
  section: string;
}

export interface DesignSessionStatus {
  shape: ProjectShape;
  movements: MovementId[];
  ratified: MovementId[];
  current: MovementId | null;
  complete: boolean;
  artifacts: DesignArtifact[];
}

// The backlog shapes, mirroring tutti-design's serde types (optional fields carry
// `#[serde(default)]` on the Rust side).
export interface ProposedIssue {
  title: string;
  body: string;
  labels?: string[];
  acceptance?: string[];
  deps?: string[];
}
export interface ProposedEpic {
  title: string;
  body: string;
  issues: ProposedIssue[];
}
export interface ProposedMilestone {
  title: string;
  due?: string | null;
  description?: string;
}
export interface BacklogPlan {
  milestone?: ProposedMilestone | null;
  epics?: ProposedEpic[];
  loose_issues?: ProposedIssue[];
}

export interface BacklogProposal {
  plan: BacklogPlan;
  rendered: string;
}

export interface SeedReport {
  created: string[];
  skipped: string[];
}

// One line of the facilitation transcript. An agent "text" bubble is the live-streaming one
// that deltas accumulate into; it is finalized into a "question" or a "section" once the
// authoritative step arrives.
export type DesignMessageKind = "text" | "question" | "section";
export interface DesignMessage {
  role: "user" | "agent";
  kind: DesignMessageKind;
  text: string;
}

// Open a fresh agent text bubble that streamed deltas will accumulate into.
export function startAgent(msgs: DesignMessage[]): DesignMessage[] {
  return [...msgs, { role: "agent", kind: "text", text: "" }];
}

// Append streamed text to the trailing open agent text bubble; otherwise open one first
// (defensive: a delta arriving with no bubble still shows).
export function appendDelta(msgs: DesignMessage[], text: string): DesignMessage[] {
  const last = msgs[msgs.length - 1];
  if (last && last.role === "agent" && last.kind === "text") {
    return [...msgs.slice(0, -1), { ...last, text: last.text + text }];
  }
  return [...msgs, { role: "agent", kind: "text", text }];
}

// Record the human's answer to the agent's last question.
export function appendAnswer(msgs: DesignMessage[], answer: string): DesignMessage[] {
  return [...msgs, { role: "user", kind: "text", text: answer }];
}

// Finalize the agent's turn as a clean question, replacing the trailing streamed text bubble
// (whose raw JSON is not worth showing) if there is one, else appending.
export function appendQuestion(msgs: DesignMessage[], question: string): DesignMessage[] {
  return replaceTrailingAgentText(msgs, { role: "agent", kind: "question", text: question });
}

// Finalize the agent's turn as a proposed artifact section, same replace-or-append rule.
export function appendRatified(msgs: DesignMessage[], section: string): DesignMessage[] {
  return replaceTrailingAgentText(msgs, { role: "agent", kind: "section", text: section });
}

// Replace a trailing open agent text bubble with `message`, or append `message` when the
// trailing bubble is not one (so a turn that streamed nothing still records its result).
function replaceTrailingAgentText(msgs: DesignMessage[], message: DesignMessage): DesignMessage[] {
  const last = msgs[msgs.length - 1];
  if (last && last.role === "agent" && last.kind === "text") {
    return [...msgs.slice(0, -1), message];
  }
  return [...msgs, message];
}

// Drop a trailing empty agent text bubble (left when a turn opened one but errored before any
// delta or a finalized reply landed).
export function dropTrailingEmptyAgent(msgs: DesignMessage[]): DesignMessage[] {
  const last = msgs[msgs.length - 1];
  if (last && last.role === "agent" && last.kind === "text" && last.text === "") {
    return msgs.slice(0, -1);
  }
  return msgs;
}

// What the pane should show given a command result. `advanced` is a transient state the pane
// resolves by beginning the next movement, so it renders nothing of its own; the mode is kept
// so this mapping is total over every DesignStep.
export type StepMode = "question" | "ratify" | "advanced" | "complete";
export interface StepUi {
  mode: StepMode;
  text: string;
}

export function stepToUi(step: DesignStep): StepUi {
  switch (step.kind) {
    case "question":
      return { mode: "question", text: step.question };
    case "ratify":
      return { mode: "ratify", text: step.artifact_section };
    case "advanced":
      return { mode: "advanced", text: "" };
    case "complete":
      return { mode: "complete", text: "" };
  }
}
