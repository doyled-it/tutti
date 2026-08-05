// SPDX-License-Identifier: AGPL-3.0-or-later
// Typed IPC wrappers over the Tauri commands, mirroring the Rust serde types in
// crates/tutti-app-core/src/lib.rs and tutti-core/src/events.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Proposal } from "./orchestrator";

export type Status = "ready" | "in_progress" | "done" | "untriaged" | "needs_human";

export interface IssueCard {
  id: number;
  title: string;
  status: Status;
  milestone: string | null;
}

export interface MilestoneRow {
  id: number;
  title: string;
  open: boolean;
  total: number;
  done: number;
}

export interface Board {
  milestones: MilestoneRow[];
  selected_milestone: number | null;
  ready: IssueCard[];
  in_progress: IssueCard[];
  done: IssueCard[];
  untriaged: IssueCard[];
  needs_human: IssueCard[];
}

/** Which triage decision to apply to a selection of issues. */
export type TriageTarget = "ready" | "needs_human";

export interface TriageFailure {
  issue: number;
  error: string;
}

/** One proposed issue resolved against the forge, so a card can be judged. */
export interface TriagePreview {
  id: number;
  /** Real title, or null when the issue could not be found. */
  title: string | null;
  status: Status | null;
  /** False when applying would be refused or is a no-op. */
  eligible: boolean;
}

/** Per-issue accounting of a triage apply: a long backlog can fail partway. */
export interface TriageOutcome {
  applied: number[];
  /** Requested but not eligible when re-read: already labelled, or gone. */
  skipped: number[];
  failed: TriageFailure[];
}

export interface LabelChip {
  name: string;
  color: string;
}

export interface IssueDetail {
  id: number;
  title: string;
  body: string;
  labels: LabelChip[];
  milestone: string | null;
  status: Status;
  branch: string;
}

export interface ProjectEntry {
  dir: string;
  repo: string;
  name: string;
  forge: string;
}

export interface ProjectList {
  projects: ProjectEntry[];
  active: string | null;
}

export interface Probe {
  has_config: boolean;
  repo: string | null;
  forge_kind: string | null;
}

export interface InitForm {
  dir: string;
  repo: string;
  forge_kind: string;
  login: string | null;
  trunk: string;
  routing: string;
  integration_branch: string;
  model: string;
  max_issues_per_run: number;
  require_label: string;
  skip_labels: string[];
  gate_commands: string[];
}

export type MessageKind = "text" | "tool" | "proposal";
export interface TranscriptMessage {
  role: "user" | "assistant";
  text: string;
  // Optional to mirror the Rust `#[serde(default)]`: a legacy transcript file written
  // without `kind` decodes with the field absent. Callers default it to "text".
  kind?: MessageKind;
}
export interface OrchestratorTranscript {
  session_id: string | null;
  messages: TranscriptMessage[];
}

// The proposal shapes live in $lib/orchestrator alongside the reducer that consumes them,
// re-exported here so the IPC surface stays the single import for callers.
export type { GateProposal, TriageProposal, Proposal } from "./orchestrator";

export interface GateStatus {
  commands: string[];
  is_noop: boolean;
}

export type NamespaceKind = "User" | "Org" | "Group";
export interface Namespace {
  path: string;
  name: string;
  kind: NamespaceKind;
}
export interface RemoteRepo {
  full_path: string;
  name: string;
  description: string | null;
  clone_url: string;
  private: boolean;
  archived: boolean;
}

export interface NewRepo {
  name: string;
  description: string | null;
  private: boolean;
}

// Discriminated union mirroring EngineEvent (serde tag = "kind", snake_case).
export type EngineEvent =
  | { kind: "drain_started" }
  | { kind: "issue_claimed"; id: number; title: string }
  | { kind: "issue_shipped"; id: number }
  | { kind: "issue_released"; id: number }
  | { kind: "drain_complete"; shipped: number };

// Mirrors tutti_core::message::Role (serde snake_case).
export type Role = "implementer" | "reviewer" | "fix_applier" | "planner";

// Discriminated union mirroring SubsessionEvent (serde tag = "kind", snake_case).
export type SubsessionEvent =
  | { kind: "started"; issue: number; role: Role; title: string }
  | { kind: "delta"; issue: number; role: Role; text: string }
  | { kind: "tool"; issue: number; role: Role; name: string }
  | { kind: "completed"; issue: number; role: Role; summary: string; ok: boolean };

export const api = {
  listProjects: () => invoke<ProjectList>("list_projects"),
  addProject: (dir: string, repo?: string) =>
    invoke<ProjectEntry>("add_project", { dir, repo: repo ?? null }),
  switchProject: (dir: string) => invoke<void>("switch_project", { dir }),
  probeProject: (dir: string) => invoke<Probe>("probe_project", { dir }),
  initProject: (form: InitForm) => invoke<ProjectEntry>("init_project", { form }),
  previewTuttiToml: (form: InitForm) => invoke<string>("preview_tutti_toml", { form }),
  removeProject: (dir: string) => invoke<void>("remove_project", { dir }),
  getBoard: (milestone?: number) => invoke<Board>("get_board", { milestone: milestone ?? null }),
  getIssue: (id: number) => invoke<IssueDetail>("get_issue", { id }),
  applyTriage: (issues: number[], to: TriageTarget) =>
    invoke<TriageOutcome>("apply_triage", { issues, to }),
  previewTriage: (issues: number[], to: TriageTarget) =>
    invoke<TriagePreview[]>("preview_triage", { issues, to }),
  startRun: () => invoke<void>("start_run"),
  pauseRun: () => invoke<void>("pause_run"),
  onProgress: (cb: (ev: EngineEvent) => void) =>
    listen<EngineEvent>("engine://progress", (e) => cb(e.payload)),
  onSubsession: (cb: (ev: SubsessionEvent) => void) =>
    listen<SubsessionEvent>("subsession://event", (e) => cb(e.payload)),
  // Fired once when a whole run ends (any exit path, including error), so the UI can
  // leave the running state even when no terminal DrainComplete was emitted.
  onRunEnded: (cb: () => void) => listen("engine://run-ended", () => cb()),
  getTranscript: () => invoke<OrchestratorTranscript>("get_transcript"),
  sendOrchestratorMessage: (message: string) =>
    invoke<void>("send_orchestrator_message", { message }),
  onOrchestratorDelta: (cb: (text: string) => void) =>
    listen<string>("orchestrator://delta", (e) => cb(e.payload)),
  onOrchestratorTool: (cb: (name: string) => void) =>
    listen<string>("orchestrator://tool", (e) => cb(e.payload)),
  // Turn-complete signal only (the streamed deltas are the source of truth for the
  // transcript, and the authoritative reply is persisted backend-side). The payload text is
  // intentionally not consumed here.
  onOrchestratorDone: (cb: () => void) => listen("orchestrator://done", () => cb()),
  onOrchestratorError: (cb: (msg: string) => void) =>
    listen<string>("orchestrator://error", (e) => cb(e.payload)),
  onOrchestratorProposal: (cb: (p: Proposal) => void) =>
    listen<Proposal>("orchestrator://proposal", (e) => cb(e.payload)),
  applyGate: (commands: string[]) => invoke<GateStatus>("apply_gate", { commands }),
  getGateStatus: () => invoke<GateStatus>("get_gate_status"),
  listNamespaces: (forgeKind: string, login: string | null) =>
    invoke<Namespace[]>("list_namespaces", { forgeKind, login }),
  listRepos: (forgeKind: string, login: string | null, namespace: Namespace) =>
    invoke<RemoteRepo[]>("list_repos", { forgeKind, login, namespace }),
  cloneRepo: (cloneUrl: string, parentDir: string, name: string) =>
    invoke<string>("clone_repo", { cloneUrl, parentDir, name }),
  createRepo: (forgeKind: string, login: string | null, namespace: Namespace, spec: NewRepo) =>
    invoke<RemoteRepo>("create_repo", { forgeKind, login, namespace, spec }),
};
