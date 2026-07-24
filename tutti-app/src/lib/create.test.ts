// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import { createSteps, validateName, validateCreateStep } from "./create";

describe("validateName", () => {
  it("rejects empty", () => expect(validateName("  ")).not.toBeNull());
  it("rejects spaces", () => expect(validateName("my repo")).not.toBeNull());
  it("rejects slashes", () => expect(validateName("a/b")).not.toBeNull());
  it("rejects odd characters", () => expect(validateName("a*b")).not.toBeNull());
  it("accepts a legal name", () => expect(validateName("my-repo_1.0")).toBeNull());
});

describe("validateCreateStep", () => {
  const base = { forgeKind: "github", login: "", name: "" };
  it("gitea forge needs a login", () =>
    expect(validateCreateStep({ ...base, forgeKind: "gitea" }, "forge")).not.toBeNull());
  it("github forge is fine", () => expect(validateCreateStep(base, "forge")).toBeNull());
  it("details step validates the name", () =>
    expect(validateCreateStep(base, "details")).not.toBeNull());
  it("details step passes a good name", () =>
    expect(validateCreateStep({ ...base, name: "widget" }, "details")).toBeNull());
  it("namespace and destination have no gate", () => {
    expect(validateCreateStep(base, "namespace")).toBeNull();
    expect(validateCreateStep(base, "destination")).toBeNull();
  });
});

describe("createSteps", () => {
  it("is forge then namespace then details then destination", () =>
    expect(createSteps()).toEqual(["forge", "namespace", "details", "destination"]));
});
