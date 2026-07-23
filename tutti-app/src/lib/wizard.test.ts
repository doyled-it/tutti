// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  initialState,
  validateStep,
  validateAll,
  stepsFor,
  toInitForm,
  MAX_ISSUES_PER_RUN,
  NO_OP_GATE,
  REQUIRE_LABEL,
  SKIP_LABELS,
  type WizardState,
} from "./wizard";
import type { Probe } from "./ipc";

const DETECTED: Probe = { has_config: false, repo: "o/r", forge_kind: "github" };
const UNDETECTED: Probe = { has_config: false, repo: null, forge_kind: null };

function base(): WizardState {
  return initialState("/tmp/proj", DETECTED);
}

describe("initialState", () => {
  it("pre-fills from the probe", () => {
    const s = base();
    expect(s.dir).toBe("/tmp/proj");
    expect(s.repo).toBe("o/r");
    expect(s.forgeKind).toBe("github");
  });

  it("falls back when the probe detected nothing", () => {
    const s = initialState("/tmp/p", UNDETECTED);
    expect(s.repo).toBe("");
    expect(s.forgeKind).toBe("github");
  });

  it("seeds the values the wizard never asks about", () => {
    const s = base();
    expect(s.requireLabel).toBe(REQUIRE_LABEL);
    expect(s.skipLabels).toEqual(SKIP_LABELS);
    expect(s.gateCommands).toEqual([NO_OP_GATE]);
    expect(s.maxIssuesPerRun).toBe(MAX_ISSUES_PER_RUN);
  });

  it("does not alias the shared skip-label constant", () => {
    const s = base();
    s.skipLabels.push("mutated");
    expect(SKIP_LABELS).toEqual(["status:needs-human"]);
  });

  it("uses the documented branch and model defaults", () => {
    const s = base();
    expect(s.trunk).toBe("main");
    expect(s.routing).toBe("trunk");
    expect(s.integrationBranch).toBe("staging");
    expect(s.model).toBe("claude-sonnet-5");
  });
});

describe("stepsFor", () => {
  it("skips the forge and repo questions when the remote answered them", () => {
    expect(stepsFor(base(), DETECTED)).toEqual(["folder", "trunk", "routing", "model", "review"]);
  });

  it("asks them when the remote did not", () => {
    const s = initialState("/tmp/p", UNDETECTED);
    expect(stepsFor(s, UNDETECTED)).toEqual([
      "folder",
      "forge",
      "repo",
      "trunk",
      "routing",
      "model",
      "review",
    ]);
  });

  it("keeps the forge step for gitea even when detected, to collect the login", () => {
    const probe: Probe = { has_config: false, repo: "o/r", forge_kind: "gitea" };
    const s = initialState("/tmp/p", probe);
    expect(stepsFor(s, probe)).toContain("forge");
    expect(stepsFor(s, probe)).not.toContain("repo");
  });

  it("drops the forge step again when the user moves off gitea", () => {
    const probe: Probe = { has_config: false, repo: "o/r", forge_kind: "gitea" };
    const s = { ...initialState("/tmp/p", probe), forgeKind: "github" };
    expect(stepsFor(s, probe)).not.toContain("forge");
  });
});

describe("validateStep", () => {
  it("accepts the default state on every step", () => {
    const s = base();
    for (const id of stepsFor(s, DETECTED)) expect(validateStep(s, id)).toBeNull();
  });

  it("requires a gitea login", () => {
    const s = { ...base(), forgeKind: "gitea", login: "" };
    expect(validateStep(s, "forge")).toMatch(/tea login/);
    expect(validateStep({ ...s, login: "codeberg" }, "forge")).toBeNull();
  });

  it("does not require a login for github", () => {
    expect(validateStep({ ...base(), login: "" }, "forge")).toBeNull();
  });

  it("rejects a malformed repo", () => {
    for (const repo of ["", "noslash", "/leading", "trailing/", "has space/x"]) {
      expect(validateStep({ ...base(), repo }, "repo")).not.toBeNull();
    }
    expect(validateStep({ ...base(), repo: "group/sub/proj" }, "repo")).toBeNull();
  });

  it("rejects an empty or whitespace trunk", () => {
    expect(validateStep({ ...base(), trunk: "" }, "trunk")).not.toBeNull();
    expect(validateStep({ ...base(), trunk: "a b" }, "trunk")).not.toBeNull();
  });

  it("rejects an integration branch equal to trunk", () => {
    const s = { ...base(), trunk: "main", integrationBranch: "main" };
    expect(validateStep(s, "routing")).toMatch(/different/);
  });

  it("rejects an empty integration branch only when routing is trunk", () => {
    expect(validateStep({ ...base(), integrationBranch: "" }, "routing")).not.toBeNull();
    expect(
      validateStep({ ...base(), routing: "phase_stacking", integrationBranch: "" }, "routing"),
    ).toBeNull();
  });

  it("rejects an empty model", () => {
    expect(validateStep({ ...base(), model: "  " }, "model")).not.toBeNull();
  });

  it("never blocks on the values the wizard no longer asks about", () => {
    const s = { ...base(), gateCommands: [], requireLabel: "", maxIssuesPerRun: 0 };
    for (const id of stepsFor(s, DETECTED)) expect(validateStep(s, id)).toBeNull();
  });
});

describe("validateAll", () => {
  it("catches a bad value on a step that was skipped", () => {
    // The repo step is hidden because detection succeeded, so only validateAll can
    // catch a detected slug that is malformed.
    expect(validateAll({ ...base(), repo: "no-slash" })).toMatch(/owner\/repo/);
  });

  it("passes on the default state", () => {
    expect(validateAll(base())).toBeNull();
  });
});

describe("toInitForm", () => {
  it("trims and drops blanks", () => {
    const f = toInitForm({
      ...base(),
      repo: "  o/r  ",
      trunk: " main ",
      integrationBranch: " staging ",
      model: " m ",
      skipLabels: [" keep ", "  ", ""],
    });
    expect(f.repo).toBe("o/r");
    expect(f.trunk).toBe("main");
    expect(f.integration_branch).toBe("staging");
    expect(f.model).toBe("m");
    expect(f.skip_labels).toEqual(["keep"]);
  });

  it("sends a login only for gitea", () => {
    expect(toInitForm({ ...base(), forgeKind: "gitea", login: " codeberg " }).login).toBe(
      "codeberg",
    );
    expect(toInitForm({ ...base(), forgeKind: "gitea", login: "  " }).login).toBeNull();
    expect(toInitForm({ ...base(), forgeKind: "github", login: "ignored" }).login).toBeNull();
  });

  it("never sends an empty gate", () => {
    expect(toInitForm({ ...base(), gateCommands: [] }).gate_commands).toEqual([NO_OP_GATE]);
    expect(toInitForm({ ...base(), gateCommands: [" "] }).gate_commands).toEqual([NO_OP_GATE]);
  });

  it("carries the remaining fields through unchanged", () => {
    const f = toInitForm({ ...base(), routing: "phase_stacking" });
    expect(f.dir).toBe("/tmp/proj");
    expect(f.forge_kind).toBe("github");
    expect(f.routing).toBe("phase_stacking");
    expect(f.max_issues_per_run).toBe(MAX_ISSUES_PER_RUN);
    expect(f.require_label).toBe(REQUIRE_LABEL);
  });
});
