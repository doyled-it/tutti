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
  | { kind: "question"; question: string; options: string[] }
  | { kind: "ratify"; artifact_section: string }
  | { kind: "advanced"; movement: MovementId; next: MovementId }
  | { kind: "complete"; movement: MovementId };

export interface DesignArtifact {
  movement: MovementId;
  section: string;
}

// The in-flight movement's pending state, so a pane reloaded mid-movement can rehydrate the
// step instead of re-running the movement's opening turn.
export interface DesignActive {
  movement: MovementId;
  pending_artifact: string | null;
  pending_question: string | null;
  // Selectable options offered with the pending question (empty for an open question), so a
  // reloaded pane re-renders the choices.
  pending_options: string[];
  // The whole in-flight movement transcript, so a reloaded pane can repaint the conversation
  // instead of showing only the pending question over a blank scrollback.
  transcript: DesignTurn[];
}

export interface DesignTurn {
  role: "agent" | "human";
  text: string;
}

export interface DesignSessionStatus {
  shape: ProjectShape;
  movements: MovementId[];
  ratified: MovementId[];
  current: MovementId | null;
  complete: boolean;
  artifacts: DesignArtifact[];
  active: DesignActive | null;
}

// The outcome of design_start: a fresh session's status, or a machine-readable signal that a
// session already exists so the caller must confirm an overwrite (tagged by `kind`, not prose).
export type DesignStartOutcome =
  { kind: "started"; status: DesignSessionStatus } | { kind: "exists_needs_overwrite" };

// The DesignStep to rehydrate from a reloaded status's `active` (a pending artifact awaits
// ratification; otherwise a pending question awaits an answer). Null when nothing is in flight.
export function activeToStep(active: DesignActive | null): DesignStep | null {
  if (!active) return null;
  if (active.pending_artifact !== null) {
    return { kind: "ratify", artifact_section: active.pending_artifact };
  }
  if (active.pending_question !== null) {
    return { kind: "question", question: active.pending_question, options: active.pending_options };
  }
  return null;
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
  /** The project deferred its stack to the design chat; offer a scaffold step before seed. */
  scaffold_pending: boolean;
}

export interface SeedReport {
  created: string[];
  skipped: string[];
}

export interface ScaffoldReport {
  stack: string;
  written: number;
  skipped: number;
  warnings: string[];
  /** Whether the scaffold was committed and pushed. False means it is on disk but not
   * published (a git failure); the pane keeps the retry path open. */
  pushed: boolean;
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

// Rebuild the transcript bubbles from a reloaded session's in-flight movement turns, so a pane
// remount (a hot reload, reopening the section) shows the whole conversation instead of a blank
// scroll. Human turns become answers. Agent turns become `question` bubbles (never `text`, which
// the template hides as the live JSON-stream placeholder), except the trailing agent turn when an
// artifact is pending: it is the proposed section and is marked `section` so its reload styling
// matches the live `appendRatified` path (monospace).
export function messagesFromActive(active: DesignActive | null): DesignMessage[] {
  if (!active) return [];
  const lastAgentIdx =
    active.pending_artifact !== null
      ? active.transcript.map((t) => t.role).lastIndexOf("agent")
      : -1;
  return active.transcript.map((t, i) => {
    if (t.role === "human") return { role: "user", kind: "text", text: t.text };
    const kind: DesignMessageKind = i === lastAgentIdx ? "section" : "question";
    return { role: "agent", kind, text: t.text };
  });
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
    default:
      // Exhaustiveness: a new DesignStep kind fails the type-check here rather than
      // returning undefined silently.
      return assertNever(step);
  }
}

function assertNever(x: never): never {
  throw new Error(`unhandled DesignStep kind: ${JSON.stringify(x)}`);
}
