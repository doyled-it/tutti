// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  initialState,
  validateStep,
  toInitForm,
  STEP_COUNT,
  MAX_ISSUES_CEILING,
  type WizardState,
} from "./wizard";

function base(): WizardState {
  return initialState("/tmp/proj", {
    has_config: false,
    repo: "o/r",
    forge_kind: "github",
  });
}

describe("initialState", () => {
  it("pre-fills from the probe", () => {
    const s = base();
    expect(s.dir).toBe("/tmp/proj");
    expect(s.repo).toBe("o/r");
    expect(s.forgeKind).toBe("github");
  });

  it("falls back when the probe detected nothing", () => {
    const s = initialState("/tmp/p", {
      has_config: false,
      repo: null,
      forge_kind: null,
    });
    expect(s.repo).toBe("");
    expect(s.forgeKind).toBe("github");
  });

  it("uses the documented defaults", () => {
    const s = base();
    expect(s.trunk).toBe("main");
    expect(s.routing).toBe("trunk");
    expect(s.integrationBranch).toBe("staging");
    expect(s.model).toBe("claude-sonnet-5");
    expect(s.maxIssuesPerRun).toBe(25);
    expect(s.requireLabel).toBe("status:ready");
    expect(s.skipLabels).toEqual(["status:needs-human"]);
    expect(s.gateCommands).toEqual(["true"]);
  });
});

describe("validateStep", () => {
  it("accepts the default state on every step", () => {
    const s = base();
    for (let i = 0; i < STEP_COUNT; i++) expect(validateStep(s, i)).toBeNull();
  });

  it("requires a gitea login", () => {
    const s = { ...base(), forgeKind: "gitea", login: "" };
    expect(validateStep(s, 1)).toMatch(/tea login/);
    expect(validateStep({ ...s, login: "codeberg" }, 1)).toBeNull();
  });

  it("does not require a login for github", () => {
    expect(validateStep({ ...base(), login: "" }, 1)).toBeNull();
  });

  it("rejects a malformed repo", () => {
    for (const repo of ["", "noslash", "/leading", "trailing/", "has space/x"]) {
      expect(validateStep({ ...base(), repo }, 2)).not.toBeNull();
    }
    expect(validateStep({ ...base(), repo: "group/sub/proj" }, 2)).toBeNull();
  });

  it("rejects an empty or whitespace trunk", () => {
    expect(validateStep({ ...base(), trunk: "" }, 3)).not.toBeNull();
    expect(validateStep({ ...base(), trunk: "a b" }, 3)).not.toBeNull();
  });

  it("rejects an integration branch equal to trunk", () => {
    const s = { ...base(), trunk: "main", integrationBranch: "main" };
    expect(validateStep(s, 4)).toMatch(/different/);
  });

  it("rejects an empty integration branch only when routing is trunk", () => {
    expect(validateStep({ ...base(), integrationBranch: "" }, 4)).not.toBeNull();
    expect(
      validateStep({ ...base(), routing: "phase_stacking", integrationBranch: "" }, 4),
    ).toBeNull();
  });

  it("rejects an empty model", () => {
    expect(validateStep({ ...base(), model: "  " }, 5)).not.toBeNull();
  });

  it("rejects an empty or blank gate command list", () => {
    expect(validateStep({ ...base(), gateCommands: [] }, 6)).not.toBeNull();
    expect(validateStep({ ...base(), gateCommands: ["cargo test", " "] }, 6)).not.toBeNull();
  });

  it("rejects an empty require label or a blank skip label", () => {
    expect(validateStep({ ...base(), requireLabel: "" }, 7)).not.toBeNull();
    expect(validateStep({ ...base(), skipLabels: ["ok", ""] }, 7)).not.toBeNull();
  });

  it("rejects a max-issues value below one", () => {
    expect(validateStep({ ...base(), maxIssuesPerRun: 0 }, 8)).not.toBeNull();
    expect(validateStep({ ...base(), maxIssuesPerRun: 1 }, 8)).toBeNull();
  });

  it("rejects a max-issues value the backend's u32 cannot hold", () => {
    expect(validateStep({ ...base(), maxIssuesPerRun: MAX_ISSUES_CEILING }, 8)).toBeNull();
    expect(validateStep({ ...base(), maxIssuesPerRun: MAX_ISSUES_CEILING + 1 }, 8)).not.toBeNull();
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
      requireLabel: " status:ready ",
      skipLabels: [" keep ", "  ", ""],
      gateCommands: [" cargo test ", " "],
    });
    expect(f.repo).toBe("o/r");
    expect(f.trunk).toBe("main");
    expect(f.integration_branch).toBe("staging");
    expect(f.model).toBe("m");
    expect(f.require_label).toBe("status:ready");
    expect(f.skip_labels).toEqual(["keep"]);
    expect(f.gate_commands).toEqual(["cargo test"]);
  });

  it("sends a login only for gitea", () => {
    expect(toInitForm({ ...base(), forgeKind: "gitea", login: " codeberg " }).login).toBe(
      "codeberg",
    );
    expect(toInitForm({ ...base(), forgeKind: "gitea", login: "  " }).login).toBeNull();
    expect(toInitForm({ ...base(), forgeKind: "github", login: "ignored" }).login).toBeNull();
  });

  it("carries the remaining fields through unchanged", () => {
    const f = toInitForm({ ...base(), routing: "phase_stacking", maxIssuesPerRun: 3 });
    expect(f.dir).toBe("/tmp/proj");
    expect(f.forge_kind).toBe("github");
    expect(f.routing).toBe("phase_stacking");
    expect(f.max_issues_per_run).toBe(3);
  });
});
