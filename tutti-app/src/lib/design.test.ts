// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect } from "vitest";
import {
  startAgent,
  appendDelta,
  appendAnswer,
  appendQuestion,
  appendRatified,
  dropTrailingEmptyAgent,
  stepToUi,
  type DesignMessage,
  type DesignStep,
} from "./design";

describe("design transcript reducers", () => {
  it("accumulates deltas into the open agent bubble", () => {
    let msgs: DesignMessage[] = [{ role: "user", kind: "text", text: "privacy" }];
    msgs = startAgent(msgs);
    msgs = appendDelta(msgs, "Hello ");
    msgs = appendDelta(msgs, "world");
    expect(msgs).toHaveLength(2);
    expect(msgs[1]).toEqual({ role: "agent", kind: "text", text: "Hello world" });
  });

  it("returns a new array rather than mutating in place", () => {
    const msgs: DesignMessage[] = [{ role: "agent", kind: "text", text: "" }];
    const next = appendDelta(msgs, "x");
    expect(next).not.toBe(msgs);
    expect(msgs[0].text).toBe("");
  });

  it("finalizes a streamed bubble into a clean question", () => {
    let msgs: DesignMessage[] = [];
    msgs = startAgent(msgs);
    msgs = appendDelta(msgs, '{"ask":"Who is this for?"}');
    msgs = appendQuestion(msgs, "Who is this for?");
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toEqual({ role: "agent", kind: "question", text: "Who is this for?" });
  });

  it("finalizes a streamed bubble into a ratifiable section", () => {
    let msgs: DesignMessage[] = [];
    msgs = startAgent(msgs);
    msgs = appendDelta(msgs, "streamed json");
    msgs = appendRatified(msgs, "## Constitution\nprivacy first");
    expect(msgs).toHaveLength(1);
    expect(msgs[0]).toEqual({
      role: "agent",
      kind: "section",
      text: "## Constitution\nprivacy first",
    });
  });

  it("appends a question when there is no streamed bubble to replace", () => {
    const msgs: DesignMessage[] = [{ role: "user", kind: "text", text: "hi" }];
    const next = appendQuestion(msgs, "What is out of scope?");
    expect(next).toHaveLength(2);
    expect(next[1]).toEqual({ role: "agent", kind: "question", text: "What is out of scope?" });
  });

  it("records a human answer as a user text message", () => {
    const msgs = appendAnswer([], "solo developers");
    expect(msgs).toEqual([{ role: "user", kind: "text", text: "solo developers" }]);
  });

  it("drops a trailing empty agent bubble but leaves a non-empty one", () => {
    const empty: DesignMessage[] = [{ role: "agent", kind: "text", text: "" }];
    expect(dropTrailingEmptyAgent(empty)).toEqual([]);
    const full: DesignMessage[] = [{ role: "agent", kind: "text", text: "partial" }];
    expect(dropTrailingEmptyAgent(full)).toEqual(full);
  });
});

describe("stepToUi", () => {
  it("maps a question step to question mode with the question text", () => {
    const step: DesignStep = { kind: "question", question: "Who?" };
    expect(stepToUi(step)).toEqual({ mode: "question", text: "Who?" });
  });

  it("maps a ratify step to ratify mode with the section text", () => {
    const step: DesignStep = { kind: "ratify", artifact_section: "## Frame" };
    expect(stepToUi(step)).toEqual({ mode: "ratify", text: "## Frame" });
  });

  it("maps an advanced step to advanced mode", () => {
    const step: DesignStep = { kind: "advanced", movement: "constitution", next: "frame" };
    expect(stepToUi(step)).toEqual({ mode: "advanced", text: "" });
  });

  it("maps a complete step to complete mode", () => {
    const step: DesignStep = { kind: "complete", movement: "decompose" };
    expect(stepToUi(step)).toEqual({ mode: "complete", text: "" });
  });
});
