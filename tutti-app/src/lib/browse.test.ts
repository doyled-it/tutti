// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import { filterRepos, cloneTarget, browseSteps, validateBrowseStep } from "./browse";
import type { RemoteRepo } from "./ipc";

const repos: RemoteRepo[] = [
  {
    full_path: "o/alpha",
    name: "alpha",
    description: "the first",
    clone_url: "",
    private: false,
    archived: false,
  },
  {
    full_path: "o/beta",
    name: "beta",
    description: null,
    clone_url: "",
    private: true,
    archived: false,
  },
];

describe("filterRepos", () => {
  it("matches name, path and description case-insensitively", () => {
    expect(filterRepos(repos, "ALPHA").map((r) => r.name)).toEqual(["alpha"]);
    expect(filterRepos(repos, "first").map((r) => r.name)).toEqual(["alpha"]);
    expect(filterRepos(repos, "o/").length).toBe(2);
  });
  it("returns all on an empty query", () => {
    expect(filterRepos(repos, "  ").length).toBe(2);
  });
});

describe("cloneTarget", () => {
  it("joins parent and repo name", () => {
    expect(cloneTarget("/home/me/code", "alpha")).toBe("/home/me/code/alpha");
    expect(cloneTarget("/home/me/code/", "alpha")).toBe("/home/me/code/alpha");
  });
});

describe("browseSteps / validateBrowseStep", () => {
  it("requires a gitea login before leaving the forge step", () => {
    expect(validateBrowseStep({ forgeKind: "gitea", login: "" }, "forge")).not.toBeNull();
    expect(validateBrowseStep({ forgeKind: "gitea", login: "x" }, "forge")).toBeNull();
    expect(validateBrowseStep({ forgeKind: "github", login: "" }, "forge")).toBeNull();
  });
});
