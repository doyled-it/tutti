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
  /**
   * Seeded high and never asked about. It bounds a single drain, and the app's run
   * driver loops drain until you pause it, so as a setup question it is noise.
   */
  maxIssuesPerRun: number;
  /**
   * Fixed convention, not a question. The engine, the board columns, and the seeded
   * forge labels all have to agree on these names, and letting setup diverge from the
   * convention buys nothing but a class of confusing mismatches.
   */
  requireLabel: string;
  skipLabels: string[];
  /**
   * Seeded to the no-op gate and never asked about. A project's real gate is not a
   * setup fact: it comes out of talking through what the project is, so it is set later
   * from the orchestrator conversation rather than guessed at here.
   */
  gateCommands: string[];
  /** True once the user picks "Custom..." on the model step, so the text input stays open. */
  modelCustom: boolean;
}

/**
 * The questions the wizard can ask, in order. Which of these actually appear depends on
 * what the folder's git remote already told us: see `stepsFor`.
 */
export type StepId = "folder" | "forge" | "repo" | "trunk" | "routing" | "model" | "review";

/** The model ids offered on the model step before falling through to a custom id. */
export const KNOWN_MODELS = ["claude-sonnet-5", "claude-opus-4-8", "claude-haiku-4-5"];

/** The gate every new project starts with: a command that always succeeds. */
export const NO_OP_GATE = "true";

/** The status label convention the engine, the board, and label seeding all share. */
export const REQUIRE_LABEL = "status:ready";
export const SKIP_LABELS = ["status:needs-human"];

/**
 * Seeded `max_issues_per_run`. High enough to be no practical limit, and still inside
 * u32 so the backend can deserialize it.
 */
export const MAX_ISSUES_PER_RUN = 1_000_000;

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
    maxIssuesPerRun: MAX_ISSUES_PER_RUN,
    requireLabel: REQUIRE_LABEL,
    skipLabels: [...SKIP_LABELS],
    gateCommands: [NO_OP_GATE],
    modelCustom: false,
  };
}

/**
 * The steps to show for this folder. Anything the probe already read off the git remote
 * is not worth asking about, so a normal clone of a GitHub repo skips straight past the
 * forge and repo questions.
 *
 * The forge step survives detection in one case: Gitea needs a `tea` login, which no
 * remote URL can tell us, so the step stays to collect it.
 */
export function stepsFor(s: WizardState, probe: Probe): StepId[] {
  const steps: StepId[] = ["folder"];
  if (probe.forge_kind === null || s.forgeKind === "gitea") steps.push("forge");
  if (probe.repo === null) steps.push("repo");
  steps.push("trunk", "routing", "model", "review");
  return steps;
}

const blank = (s: string) => s.trim().length === 0;

/**
 * Validate one step. Returns the message to show under the control, or null when the
 * step is answerable. Mirrors Config::validate so a bad value is caught on the step
 * that caused it instead of surfacing as a backend error after Create.
 */
export function validateStep(s: WizardState, step: StepId): string | null {
  switch (step) {
    case "folder":
      return blank(s.dir) ? "Choose a folder to initialize." : null;
    case "forge":
      return s.forgeKind === "gitea" && blank(s.login)
        ? "Gitea needs a `tea` login. Run `tea login list` to see yours."
        : null;
    case "repo": {
      const r = s.repo.trim();
      if (blank(r) || !r.includes("/") || r.startsWith("/") || r.endsWith("/") || /\s/.test(r)) {
        return "Enter it as `owner/repo`.";
      }
      return null;
    }
    case "trunk":
      return blank(s.trunk) || /\s/.test(s.trunk.trim()) ? "Enter a branch name." : null;
    case "routing": {
      if (s.routing === "trunk" && blank(s.integrationBranch)) {
        return "Enter the branch Tutti should merge finished work into.";
      }
      if (s.integrationBranch.trim() && s.integrationBranch.trim() === s.trunk.trim()) {
        return "The integration branch must be different from your trunk branch.";
      }
      return null;
    }
    case "model":
      return blank(s.model) ? "Enter a model id." : null;
    default:
      return null;
  }
}

/**
 * Whether every step is answerable. Guards Create against a skipped step holding a bad
 * value, which is reachable when the repo step is hidden but detection produced
 * something malformed.
 */
export function validateAll(s: WizardState): string | null {
  const ids: StepId[] = ["folder", "forge", "repo", "trunk", "routing", "model"];
  for (const id of ids) {
    const e = validateStep(s, id);
    if (e) return e;
  }
  return null;
}

/** Project the wizard state onto the backend payload. */
export function toInitForm(s: WizardState): InitForm {
  // No step can empty this, but an empty command list would mean "ship without running
  // anything", which reads the same as the no-op gate while being far less obvious in
  // the file. Keep the explicit `true` so the config always says what it does.
  const gate = s.gateCommands.map((c) => c.trim()).filter((c) => c.length > 0);
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
    gate_commands: gate.length > 0 ? gate : [NO_OP_GATE],
  };
}
