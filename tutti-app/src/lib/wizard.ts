// SPDX-License-Identifier: AGPL-3.0-or-later
// Pure state and validation for the new-project wizard. Deliberately free of Svelte and
// Tauri imports so every rule below is unit-testable without mounting anything.
import type { InitForm, Probe } from "./ipc";

export interface WizardState {
  dir: string;
  forgeKind: string;
  login: string;
  repo: string;
  trunk: string;
  routing: string;
  integrationBranch: string;
  model: string;
  maxIssuesPerRun: number;
  requireLabel: string;
  skipLabels: string[];
  gateCommands: string[];
  /** True once the user picks "Custom..." on the model step, so the text input stays open. */
  modelCustom: boolean;
}

/** How many question steps the wizard has, including the final review step. */
export const STEP_COUNT = 10;

/** The model ids offered on step 6 before falling through to a custom id. */
export const KNOWN_MODELS = ["claude-sonnet-5", "claude-opus-4-8", "claude-haiku-4-5"];

export function initialState(dir: string, probe: Probe): WizardState {
  return {
    dir,
    forgeKind: probe.forge_kind ?? "github",
    login: "",
    repo: probe.repo ?? "",
    trunk: "main",
    routing: "trunk",
    integrationBranch: "staging",
    model: "claude-sonnet-5",
    maxIssuesPerRun: 25,
    requireLabel: "status:ready",
    skipLabels: ["status:needs-human"],
    gateCommands: ["true"],
    modelCustom: false,
  };
}

const blank = (s: string) => s.trim().length === 0;

/**
 * Validate one step. Returns the message to show under the control, or null when the
 * step is answerable. Mirrors Config::validate so a bad value is caught on the step
 * that caused it instead of surfacing as a backend error after Create.
 */
export function validateStep(s: WizardState, index: number): string | null {
  switch (index) {
    case 0:
      return blank(s.dir) ? "Choose a folder to initialize." : null;
    case 1:
      return s.forgeKind === "gitea" && blank(s.login)
        ? "Gitea needs a `tea` login. Run `tea login list` to see yours."
        : null;
    case 2: {
      const r = s.repo.trim();
      if (blank(r) || !r.includes("/") || r.startsWith("/") || r.endsWith("/") || /\s/.test(r)) {
        return "Enter it as `owner/repo`.";
      }
      return null;
    }
    case 3:
      return blank(s.trunk) || /\s/.test(s.trunk.trim()) ? "Enter a branch name." : null;
    case 4: {
      if (s.routing === "trunk" && blank(s.integrationBranch)) {
        return "Enter the branch Tutti should merge finished work into.";
      }
      if (s.integrationBranch.trim() && s.integrationBranch.trim() === s.trunk.trim()) {
        return "The integration branch must be different from your trunk branch.";
      }
      return null;
    }
    case 5:
      return blank(s.model) ? "Enter a model id." : null;
    case 6:
      if (s.gateCommands.length === 0 || s.gateCommands.some(blank)) {
        return "Add at least one command, or use `true` for no gate.";
      }
      return null;
    case 7:
      if (blank(s.requireLabel)) return "Enter the label Tutti should require.";
      if (s.skipLabels.some(blank)) return "Remove the empty skip label.";
      return null;
    case 8:
      return Number.isInteger(s.maxIssuesPerRun) && s.maxIssuesPerRun >= 1
        ? null
        : "Enter a number of 1 or more.";
    default:
      return null;
  }
}

/** Project the wizard state onto the backend payload. */
export function toInitForm(s: WizardState): InitForm {
  return {
    dir: s.dir,
    repo: s.repo.trim(),
    forge_kind: s.forgeKind,
    login: s.forgeKind === "gitea" ? s.login.trim() || null : null,
    trunk: s.trunk.trim(),
    routing: s.routing,
    // phase_stacking ignores it, but the renderer always emits the key, so send the
    // value the user last saw rather than an empty string.
    integration_branch: s.integrationBranch.trim(),
    model: s.model.trim(),
    max_issues_per_run: s.maxIssuesPerRun,
    require_label: s.requireLabel.trim(),
    skip_labels: s.skipLabels.map((l) => l.trim()).filter((l) => l.length > 0),
    gate_commands: s.gateCommands.map((c) => c.trim()).filter((c) => c.length > 0),
  };
}
