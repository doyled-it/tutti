// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure helpers for the browse-a-forge flow: the step list, the repo filter, and the
// clone-target path. No Svelte or Tauri imports so all of it is unit-testable.
import type { RemoteRepo } from "./ipc";

export type BrowseStepId = "forge" | "namespace" | "repo" | "destination";

export const BROWSE_STEPS: BrowseStepId[] = ["forge", "namespace", "repo", "destination"];

export function browseSteps(): BrowseStepId[] {
  return BROWSE_STEPS;
}

/** Case-insensitive match over a repo's name, full path, and description. */
export function filterRepos(repos: RemoteRepo[], query: string): RemoteRepo[] {
  const q = query.trim().toLowerCase();
  if (!q) return repos;
  return repos.filter(
    (r) =>
      r.name.toLowerCase().includes(q) ||
      r.full_path.toLowerCase().includes(q) ||
      (r.description ?? "").toLowerCase().includes(q),
  );
}

/** The local path a repo will clone into: `<parent>/<name>`. */
export function cloneTarget(parentDir: string, name: string): string {
  const base = parentDir.replace(/\/+$/, "");
  return `${base}/${name}`;
}

/** Minimal state the browse validation needs. */
export interface BrowseFormish {
  forgeKind: string;
  login: string;
}

/** Validate one browse step; null when answerable. */
export function validateBrowseStep(s: BrowseFormish, step: BrowseStepId): string | null {
  if (step === "forge" && s.forgeKind === "gitea" && s.login.trim().length === 0) {
    return "Gitea needs a `tea` login. Run `tea login list` to see yours.";
  }
  return null;
}
