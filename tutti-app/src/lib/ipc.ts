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

export interface IssueDetail {
  id: number;
  title: string;
  body: string;
  labels: string[];
  milestone: string | null;
  status: Status;
  branch: string;
}

export interface ProjectSummary {
  name: string;
  forge: string;
  repo: string;
}

// Discriminated union mirroring EngineEvent (serde tag = "kind", snake_case).
export type EngineEvent =
  | { kind: "drain_started" }
  | { kind: "issue_claimed"; id: number; title: string }
  | { kind: "issue_shipped"; id: number }
  | { kind: "issue_released"; id: number }
  | { kind: "drain_complete"; shipped: number };

export const api = {
  loadProject: (dir: string, repo: string) =>
    invoke<ProjectSummary>("load_project", { dir, repo }),
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
