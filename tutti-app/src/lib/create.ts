// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure helpers for the create-a-new-repo flow: the step list and the name/step
// validation. No Svelte or Tauri imports so all of it is unit-testable. Reuses the
// browse flow's forge-step validator to stay DRY.
import { validateBrowseStep } from "./browse";

export type CreateStepId = "forge" | "namespace" | "details" | "destination";

export const CREATE_STEPS: CreateStepId[] = ["forge", "namespace", "details", "destination"];

export function createSteps(): CreateStepId[] {
  return CREATE_STEPS;
}

/** Validate a repo name. Returns an error string, or null when legal. */
export function validateName(name: string): string | null {
  const n = name.trim();
  if (n.length === 0) return "Enter a repository name.";
  if (/\s/.test(n)) return "A repository name cannot contain spaces.";
  if (/[/\\]/.test(n)) return "A repository name cannot contain slashes.";
  if (!/^[A-Za-z0-9._-]+$/.test(n)) {
    return "Use only letters, numbers, dot, dash, or underscore.";
  }
  return null;
}

/** Minimal state the create validation needs. */
export interface CreateFormish {
  forgeKind: string;
  login: string;
  name: string;
}

/** Validate one create step; null when the user may advance. */
export function validateCreateStep(s: CreateFormish, step: CreateStepId): string | null {
  if (step === "forge")
    return validateBrowseStep({ forgeKind: s.forgeKind, login: s.login }, "forge");
  if (step === "details") return validateName(s.name);
  return null;
}
