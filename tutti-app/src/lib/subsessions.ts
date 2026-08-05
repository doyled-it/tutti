// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure reducer for the Subsessions pane. Folds the pushed SubsessionEvent stream into a
// keyed, in-memory collection of per-role transcripts. Live-only (never persisted). Reuses
// the orchestrator chat's transcript REDUCER primitives (startAssistant/appendDelta/
// appendTool/dropTrailingEmptyAssistant); the rendering is still each pane's own. Kept
// pure and vitest-covered per the board.ts / browse.ts / orchestrator.ts convention.

import type { Role, SubsessionEvent } from "./ipc";
import {
  startAssistant,
  appendDelta,
  appendTool,
  dropTrailingEmptyAssistant,
  type ChatMessage,
} from "./orchestrator";

export type SubStatus = "running" | "done" | "error";

export interface Subsession {
  key: string; // `${issue}:${role}`
  issue: number;
  role: Role;
  title: string;
  status: SubStatus;
  messages: ChatMessage[];
  summary?: string;
}

export interface SubsessionState {
  list: Subsession[]; // insertion order (stable left-rail order)
  selected: string | null; // key
}

export function emptySubsessions(): SubsessionState {
  return { list: [], selected: null };
}

// Cap on retained subsessions so a long drain does not grow transcripts without bound. A run
// processes issues sequentially (roughly 3-4 role turns each), and eviction only ever removes
// the oldest entries that are neither running nor currently selected, so the live edge and
// anything the viewer has open are always kept.
export const MAX_SUBSESSIONS = 100;

// Caps on a single subsession's transcript. Unlike the orchestrator's human-paced chat, one
// role turn on a large repo can stream a lot: without a bound, `messages` grows forever and
// `appendDelta`'s copy-on-append makes accumulation quadratic in transcript length. Bounding
// both axes keeps every append constant-bounded.
export const MAX_TRANSCRIPT_MESSAGES = 400;
export const MAX_BUBBLE_CHARS = 20_000;

// Marks where a transcript was trimmed, so a truncated view never reads as the whole turn.
const ELISION = "[... earlier output trimmed ...]\n";

const keyOf = (issue: number, role: Role): string => `${issue}:${role}`;

const ROLE_WORDS: Record<Role, string> = {
  implementer: "Implementer",
  reviewer: "Reviewer",
  fix_applier: "Fix applier",
  planner: "Planner",
};

// Human label for a subsession row/header. The Planner runs on the sentinel issue 0, so it
// shows no issue number.
export function roleLabel(s: { issue: number; role: Role }): string {
  const words = ROLE_WORDS[s.role];
  return s.role === "planner" ? words : `#${s.issue} ${words}`;
}

// Replace the subsession at `key` (or append it), preserving list order.
function upsert(list: Subsession[], sub: Subsession): Subsession[] {
  const i = list.findIndex((s) => s.key === sub.key);
  if (i === -1) return [...list, sub];
  return [...list.slice(0, i), sub, ...list.slice(i + 1)];
}

// Evict the oldest completed, non-selected subsessions once the list exceeds MAX_SUBSESSIONS.
function capList(list: Subsession[], selected: string | null): Subsession[] {
  if (list.length <= MAX_SUBSESSIONS) return list;
  let toDrop = list.length - MAX_SUBSESSIONS;
  return list.filter((s) => {
    if (toDrop > 0 && s.status !== "running" && s.key !== selected) {
      toDrop--;
      return false;
    }
    return true;
  });
}

// Bound one subsession's transcript: drop the oldest messages past MAX_TRANSCRIPT_MESSAGES,
// and keep the trailing (live) text bubble under MAX_BUBBLE_CHARS by dropping from its front.
// Both keep the newest output, which is what a live viewer is watching. Pure: returns the
// input array untouched when nothing needs trimming.
function capTranscript(msgs: ChatMessage[]): ChatMessage[] {
  let out = msgs;
  if (out.length > MAX_TRANSCRIPT_MESSAGES) {
    out = out.slice(out.length - MAX_TRANSCRIPT_MESSAGES);
  }
  const last = out[out.length - 1];
  if (last && last.kind === "text" && last.text.length > MAX_BUBBLE_CHARS) {
    const trimmed = { ...last, text: ELISION + last.text.slice(-MAX_BUBBLE_CHARS) };
    out = [...out.slice(0, -1), trimmed];
  }
  return out;
}

// Apply `fn` to the subsession at `key`; no-op if absent.
function edit(
  state: SubsessionState,
  key: string,
  fn: (s: Subsession) => Subsession,
): SubsessionState {
  const i = state.list.findIndex((s) => s.key === key);
  if (i === -1) return state;
  const updated = fn(state.list[i]);
  return { ...state, list: [...state.list.slice(0, i), updated, ...state.list.slice(i + 1)] };
}

// Set the selected key. A small, pure setter so the pane's row click goes through the same
// reducer module as every other transition, instead of hand-rolling a store update.
export function selectSubsession(state: SubsessionState, key: string): SubsessionState {
  return { ...state, selected: key };
}

export function applySubsession(state: SubsessionState, ev: SubsessionEvent): SubsessionState {
  const key = keyOf(ev.issue, ev.role);
  switch (ev.kind) {
    case "started": {
      const sub: Subsession = {
        key,
        issue: ev.issue,
        role: ev.role,
        title: ev.title,
        status: "running",
        messages: startAssistant([]),
        summary: undefined,
      };
      // Follow the live edge: select the newest started turn.
      return { list: capList(upsert(state.list, sub), key), selected: key };
    }
    case "delta":
      return edit(state, key, (s) => ({
        ...s,
        messages: capTranscript(appendDelta(s.messages, ev.text)),
      }));
    case "tool":
      return edit(state, key, (s) => ({
        ...s,
        messages: capTranscript(appendTool(s.messages, ev.name)),
      }));
    case "completed":
      return edit(state, key, (s) => ({
        ...s,
        status: ev.ok ? "done" : "error",
        summary: ev.summary,
        messages: dropTrailingEmptyAssistant(s.messages),
      }));
  }
}
