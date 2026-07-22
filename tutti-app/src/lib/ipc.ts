// SPDX-License-Identifier: AGPL-3.0-or-later
// Typed IPC wrappers over the Tauri commands, mirroring the Rust serde types in
// crates/tutti-app-core/src/lib.rs and tutti-core/src/events.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Status = "ready" | "in_progress" | "done" | "other";

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

// Discriminated union mirroring EngineEvent (serde tag = "kind", snake_case).
export type EngineEvent =
  | { kind: "drain_started" }
  | { kind: "issue_claimed"; id: number; title: string }
  | { kind: "issue_shipped"; id: number }
  | { kind: "issue_released"; id: number }
  | { kind: "drain_complete"; shipped: number };

export const api = {
  listProjects: () => invoke<ProjectList>("list_projects"),
  addProject: (dir: string, repo?: string) =>
    invoke<ProjectEntry>("add_project", { dir, repo: repo ?? null }),
  switchProject: (dir: string) => invoke<void>("switch_project", { dir }),
  probeProject: (dir: string) => invoke<Probe>("probe_project", { dir }),
  initProject: (form: InitForm) =>
    invoke<ProjectEntry>("init_project", { form }),
  previewTuttiToml: (form: InitForm) =>
    invoke<string>("preview_tutti_toml", { form }),
  removeProject: (dir: string) => invoke<void>("remove_project", { dir }),
  getBoard: (milestone?: number) =>
    invoke<Board>("get_board", { milestone: milestone ?? null }),
  getIssue: (id: number) => invoke<IssueDetail>("get_issue", { id }),
  startRun: () => invoke<void>("start_run"),
  pauseRun: () => invoke<void>("pause_run"),
  onProgress: (cb: (ev: EngineEvent) => void) =>
    listen<EngineEvent>("engine://progress", (e) => cb(e.payload)),
  // Fired once when a whole run ends (any exit path, including error), so the UI can
  // leave the running state even when no terminal DrainComplete was emitted.
  onRunEnded: (cb: () => void) => listen("engine://run-ended", () => cb()),
};
