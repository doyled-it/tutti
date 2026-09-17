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
  activeToStep,
  messagesFromActive,
  groupBacklog,
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

  it("opens a new agent bubble when a delta arrives with none open", () => {
    // The defensive branch: a delta after a finalized question (not an open text bubble)
    // must start a fresh bubble, not append to the question.
    const msgs: DesignMessage[] = [{ role: "agent", kind: "question", text: "Who?" }];
    const next = appendDelta(msgs, "streamed");
    expect(next).toHaveLength(2);
    expect(next[1]).toEqual({ role: "agent", kind: "text", text: "streamed" });
    expect(next[0]).toEqual({ role: "agent", kind: "question", text: "Who?" });
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

describe("activeToStep (reload rehydration)", () => {
  it("rehydrates a pending artifact as a ratify step", () => {
    const step = activeToStep({
      movement: "constitution",
      pending_artifact: "## Constitution\nprivacy first",
      pending_question: null,
      pending_options: [],
      transcript: [],
    });
    expect(step).toEqual({ kind: "ratify", artifact_section: "## Constitution\nprivacy first" });
  });

  it("rehydrates a pending question with its options when no artifact is proposed", () => {
    const step = activeToStep({
      movement: "frame",
      pending_artifact: null,
      pending_question: "Who is this for?",
      pending_options: ["Solo devs", "Teams"],
      transcript: [],
    });
    expect(step).toEqual({
      kind: "question",
      question: "Who is this for?",
      options: ["Solo devs", "Teams"],
    });
  });

  it("is null when nothing is in flight", () => {
    expect(activeToStep(null)).toBeNull();
    expect(
      activeToStep({
        movement: "frame",
        pending_artifact: null,
        pending_question: null,
        pending_options: [],
        transcript: [],
      }),
    ).toBeNull();
  });
});

describe("messagesFromActive (reload repaint)", () => {
  it("is empty when nothing is in flight", () => {
    expect(messagesFromActive(null)).toEqual([]);
  });

  it("repaints human turns as answers and agent turns as questions", () => {
    const msgs = messagesFromActive({
      movement: "constitution",
      pending_artifact: null,
      pending_question: "What must stay true?",
      pending_options: [],
      transcript: [
        { role: "agent", text: "Who is this for?" },
        { role: "human", text: "MLB fans" },
        { role: "agent", text: "What must stay true?" },
      ],
    });
    expect(msgs).toEqual([
      { role: "agent", kind: "question", text: "Who is this for?" },
      { role: "user", kind: "text", text: "MLB fans" },
      { role: "agent", kind: "question", text: "What must stay true?" },
    ]);
  });

  it("never emits a hidden agent text bubble", () => {
    const msgs = messagesFromActive({
      movement: "frame",
      pending_artifact: "## Frame",
      pending_question: null,
      pending_options: [],
      transcript: [{ role: "agent", text: "a question" }],
    });
    expect(msgs.some((m) => m.role === "agent" && m.kind === "text")).toBe(false);
  });

  it("marks the trailing agent turn as a section when an artifact is pending", () => {
    const msgs = messagesFromActive({
      movement: "frame",
      pending_artifact: "## Frame\nthe frame",
      pending_question: null,
      pending_options: [],
      transcript: [
        { role: "agent", text: "a question" },
        { role: "human", text: "an answer" },
        { role: "agent", text: "## Frame\nthe frame" },
      ],
    });
    expect(msgs[0]).toEqual({ role: "agent", kind: "question", text: "a question" });
    expect(msgs[2]).toEqual({ role: "agent", kind: "section", text: "## Frame\nthe frame" });
  });
});

describe("stepToUi", () => {
  it("maps a question step to question mode with the question text", () => {
    const step: DesignStep = { kind: "question", question: "Who?", options: [] };
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

describe("groupBacklog", () => {
  it("groups issues under their milestone and folds unnamed into a sole milestone", () => {
    const { groups, looseIssues } = groupBacklog({
      milestones: [{ title: "M1" }, { title: "M2" }],
      loose_issues: [
        { title: "A", body: "", milestone: "M1" },
        { title: "B", body: "", milestone: "M2" },
        { title: "C", body: "", milestone: "M2" },
      ],
    });
    expect(groups.map((g) => g.title)).toEqual(["M1", "M2"]);
    expect(groups[0].issues.map((i) => i.title)).toEqual(["A"]);
    expect(groups[1].issues.map((i) => i.title)).toEqual(["B", "C"]);
    expect(looseIssues).toEqual([]);
  });

  it("puts an issue with an unknown milestone in loose issues", () => {
    const { looseIssues } = groupBacklog({
      milestones: [{ title: "M1" }],
      loose_issues: [{ title: "X", body: "", milestone: "Ghost" }],
    });
    expect(looseIssues.map((i) => i.title)).toEqual(["X"]);
  });
});
