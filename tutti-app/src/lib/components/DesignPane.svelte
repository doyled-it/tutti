<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Design surface: a per-movement chat that drives the design chain turn by turn, a
     live preview of the accreting design page, and a backlog review/confirm step before
     seeding. Each turn is one Tauri command (begin/reply/revise/ratify); the agent's turn
     streams in over design://delta. All transcript and step logic lives in $lib/design;
     this component is wiring only. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api } from "$lib/ipc";
  import { renderMarkdown } from "$lib/markdown";
  import { autogrow } from "$lib/autogrow";
  import { ResizableWidth } from "$lib/resizable.svelte";
  import { designBusy } from "$lib/stores";
  import Resizer from "./Resizer.svelte";
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
    type DesignMessage,
    type DesignSessionStatus,
    type DesignStep,
    type BacklogProposal,
    type SeedReport,
    type ScaffoldReport,
    type ProjectShape,
    type MovementId,
  } from "$lib/design";

  let status = $state<DesignSessionStatus | null>(null);
  let messages = $state<DesignMessage[]>([]);
  let step = $state<DesignStep | null>(null);
  let preview = $state("");
  let proposal = $state<BacklogProposal | null>(null);
  let seedReport = $state<SeedReport | null>(null);
  // The scaffold step, shown between propose and seed only when the project deferred its
  // stack to the design chat (`proposal.scaffold_pending`). Once scaffolded, `scaffolded`
  // gates it off and the Seed button appears.
  let scaffoldStack = $state("typescript");
  let scaffoldReport = $state<ScaffoldReport | null>(null);
  let scaffolded = $state(false);

  // The real (scaffoldable) stacks, matching tutti-app-core's stack ids. Mirrors the create
  // wizard minus its "defer"/"none" opt-outs, since here a concrete stack is being chosen.
  const DESIGN_STACKS: { id: string; label: string }[] = [
    { id: "python", label: "Python (uv, ruff, mypy, pytest)" },
    { id: "rust", label: "Rust (cargo fmt, clippy, test)" },
    { id: "typescript", label: "TypeScript (bun, tsc, bun test)" },
    { id: "go", label: "Go (gofmt, vet, test)" },
  ];

  function stackLabel(id: string): string {
    return DESIGN_STACKS.find((s) => s.id === id)?.label ?? id;
  }

  // The agent's recommended stack, but only when it is one the picker can actually select, so the
  // hint never names a stack the control will not reflect. Null when absent or unrecognized.
  let recommendedStack = $derived.by(() => {
    const rec = proposal?.plan.recommended_stack;
    return rec && DESIGN_STACKS.some((s) => s.id === rec) ? rec : null;
  });

  let draft = $state("");
  let reviseOpen = $state(false);
  let reviseText = $state("");
  let thinking = $state(false);
  let error = $state<string | null>(null);
  // A shape awaiting an overwrite confirmation: set when design_start reports a session already
  // exists, cleared on cancel or after the confirmed start. Guards against silently clobbering an
  // in-progress design conversation (see the backend guard in design_start).
  let startOverShape = $state<ProjectShape | null>(null);

  // Bound DOM nodes for the ergonomics fixes below.
  let transcriptEl = $state<HTMLDivElement | null>(null);
  let composerEl = $state<HTMLTextAreaElement | null>(null);

  // The draggable width of the chat pane (the preview flexes to fill the rest), persisted.
  const split = new ResizableWidth({
    key: "tutti.designChatWidth",
    min: 320,
    max: 1000,
    default: 560,
  });

  // Sticky-bottom autoscroll, mirroring SubsessionsPane: only follow the tail when the reader is
  // already near the bottom, so scrolling up to re-read a question is not fought on every
  // streamed token. `stick`/`lastMovement` are plain locals (bookkeeping across effect runs),
  // deliberately NOT $state so writing them in the effect cannot loop it.
  let stick = true;
  let lastMovement: string | null = null;

  function onTranscriptScroll() {
    const el = transcriptEl;
    if (!el) return;
    // "Near the bottom" tolerance so a reader parked at the end stays stuck through a delta.
    stick = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  }

  // Links inside rendered markdown open in the user's external browser, not the Tauri webview
  // (mirrors IssueDrawer). Applied to the section bubble's {@html} container.
  function externalLinks(node: HTMLElement) {
    const handler = (e: MouseEvent) => {
      const a = (e.target as HTMLElement)?.closest("a");
      const href = a?.getAttribute("href");
      if (a && href) {
        e.preventDefault();
        void openUrl(href);
      }
    };
    node.addEventListener("click", handler);
    node.addEventListener("auxclick", handler);
    return {
      destroy: () => {
        node.removeEventListener("click", handler);
        node.removeEventListener("auxclick", handler);
      },
    };
  }

  $effect(() => {
    const movement = status?.current ?? null;
    // Touch what grows/changes so the effect re-runs: a new/streamed bubble, the Thinking
    // indicator toggling, and the movement switching.
    void messages.length;
    void thinking;
    const el = transcriptEl;
    if (!el) return;
    if (movement !== lastMovement) {
      // New movement (or first shown): jump to the latest and re-arm sticky.
      lastMovement = movement;
      stick = true;
      el.scrollTop = el.scrollHeight;
    } else if (stick) {
      el.scrollTop = el.scrollHeight;
    }
  });

  // Return focus to the answer box whenever it becomes usable (initial question and after each
  // send), so the user does not have to click back into it after pressing Enter.
  $effect(() => {
    if (questioning && !thinking) composerEl?.focus();
  });

  // The delta listener runs for every turn, but only movement turns (begin/reply/revise)
  // finalize their stream into a clean question/section, so only they stream into the
  // transcript. Ratify runs no agent turn; propose streams raw decompose JSON not worth
  // showing. This flag gates what the transcript accumulates.
  let streaming = false;

  const SHAPES: { id: ProjectShape; label: string }[] = [
    { id: "small_cli", label: "Small CLI or library" },
    { id: "mobile", label: "Mobile app" },
    { id: "multi_service", label: "Multi-service" },
  ];

  function movementLabel(m: MovementId): string {
    return m.charAt(0).toUpperCase() + m.slice(1);
  }

  onMount(() => {
    (async () => {
      try {
        status = await api.designSessionStatus();
        if (status) {
          preview = await api.designPreview().catch(() => "");
          // Repaint the movement's conversation so a remount (hot reload, reopening the
          // section) shows the whole Q&A history instead of a blank scrollback.
          messages = messagesFromActive(status.active);
          // Rehydrate the in-flight step after a reload, so a movement awaiting ratification
          // shows its proposed section (not a Continue button that would re-run the turn and
          // overwrite it), and one awaiting an answer shows its question.
          step = activeToStep(status.active);
        }
      } catch (e) {
        error = String(e);
      }
    })();

    const unlisten = api.onDesignDelta((text) => {
      if (streaming) messages = appendDelta(messages, text);
    });
    return () => {
      unlisten.then((u) => u());
      // If the pane unmounts mid-turn, do not leave the sidebar gated forever. The backend
      // single-flight guard still prevents a second concurrent turn.
      designBusy.set(false);
    };
  });

  async function refreshStatusAndPreview() {
    try {
      status = await api.designSessionStatus();
    } catch {
      // Keep the last known status; the error path below surfaces hard failures.
    }
    try {
      preview = await api.designPreview();
    } catch {
      // A preview read failure is non-fatal; the transcript still drives the session.
    }
  }

  // Route a command result to the pane. A question or a proposed section renders and waits
  // for input; a ratified-and-advanced state begins the next movement; a completed chain
  // proposes the backlog.
  async function handleStep(st: DesignStep) {
    const ui = stepToUi(st);
    if (ui.mode === "question") {
      messages = appendQuestion(messages, ui.text);
      step = st;
    } else if (ui.mode === "ratify") {
      messages = appendRatified(messages, ui.text);
      step = st;
    } else if (ui.mode === "advanced") {
      step = null;
      await refreshStatusAndPreview();
      await begin();
    } else {
      step = null;
      await refreshStatusAndPreview();
      await propose();
    }
  }

  async function start(shape: ProjectShape) {
    error = null;
    try {
      const outcome = await api.designStart(shape);
      // A session already exists: show an overwrite confirm rather than starting over silently.
      // The signal is the outcome's `kind`, not a matched error string, so a reworded backend
      // message cannot break the confirm path.
      if (outcome.kind === "exists_needs_overwrite") {
        startOverShape = shape;
        return;
      }
      // No existing session, so `messages` is already empty (messagesFromActive only runs when a
      // status exists); no reset needed here, unlike the overwrite path.
      status = outcome.status;
      await begin();
    } catch (e) {
      error = String(e);
    }
  }

  async function startConfirmed() {
    const shape = startOverShape;
    startOverShape = null;
    if (!shape) return;
    error = null;
    try {
      // overwrite: the backend snapshots the prior session to a recovery branch first.
      const outcome = await api.designStart(shape, true);
      if (outcome.kind !== "started") return; // unreachable with overwrite=true, but total.
      // A prior session may have painted the transcript; clear it for the fresh conversation.
      messages = [];
      status = outcome.status;
      await begin();
    } catch (e) {
      error = String(e);
    }
  }

  async function begin() {
    error = null;
    thinking = true;
    streaming = true;
    designBusy.set(true);
    messages = startAgent(messages);
    try {
      const st = await api.designBeginMovement();
      await handleStep(st);
    } catch (e) {
      messages = dropTrailingEmptyAgent(messages);
      error = String(e);
    } finally {
      streaming = false;
      thinking = false;
      designBusy.set(false);
    }
  }

  // Send an explicit answer (a picked option, or the typed draft). Shared by the Send button,
  // Enter in the composer, an option click, and arrow+Enter option selection.
  async function sendAnswer(text: string) {
    const answer = text.trim();
    if (!answer || thinking) return;
    error = null;
    messages = appendAnswer(messages, answer);
    draft = "";
    optionIndex = 0;
    step = null;
    thinking = true;
    streaming = true;
    designBusy.set(true);
    messages = startAgent(messages);
    try {
      const st = await api.designReply(answer);
      await handleStep(st);
    } catch (e) {
      messages = dropTrailingEmptyAgent(messages);
      error = String(e);
    } finally {
      streaming = false;
      thinking = false;
      designBusy.set(false);
    }
  }

  function reply() {
    void sendAnswer(draft);
  }

  async function ratify() {
    if (thinking) return;
    error = null;
    step = null;
    thinking = true;
    designBusy.set(true);
    try {
      const st = await api.designRatify();
      await handleStep(st);
    } catch (e) {
      error = String(e);
    } finally {
      thinking = false;
      designBusy.set(false);
    }
  }

  async function revise() {
    const text = reviseText.trim();
    if (!text || thinking) return;
    error = null;
    reviseOpen = false;
    reviseText = "";
    step = null;
    thinking = true;
    streaming = true;
    designBusy.set(true);
    messages = appendAnswer(messages, text);
    messages = startAgent(messages);
    try {
      const st = await api.designRevise(text);
      await handleStep(st);
    } catch (e) {
      messages = dropTrailingEmptyAgent(messages);
      error = String(e);
    } finally {
      streaming = false;
      thinking = false;
      designBusy.set(false);
    }
  }

  async function propose() {
    error = null;
    seedReport = null;
    scaffoldReport = null;
    scaffolded = false;
    thinking = true;
    designBusy.set(true);
    try {
      proposal = await api.designProposeBacklog();
      // Pre-select the stack the agent recommended from the design (the user can still change
      // it), or the default when the reply carried no valid recommendation. Reset every propose
      // so a stale recommendation from a discarded proposal never lingers in the picker.
      const rec = proposal.plan.recommended_stack;
      scaffoldStack = rec && DESIGN_STACKS.some((s) => s.id === rec) ? rec : "typescript";
    } catch (e) {
      error = String(e);
    } finally {
      thinking = false;
      designBusy.set(false);
    }
  }

  async function scaffold() {
    if (!proposal || thinking) return;
    error = null;
    thinking = true;
    designBusy.set(true);
    try {
      scaffoldReport = await api.designScaffold(scaffoldStack);
      // Advance to seeding only when the scaffold was actually committed and pushed. On a git
      // failure (pushed=false) stay on the step so the warning shows and the user can retry or
      // seed anyway; the backend leaves the marker in place for a clean retry.
      scaffolded = scaffoldReport.pushed;
    } catch (e) {
      error = String(e);
    } finally {
      thinking = false;
      designBusy.set(false);
    }
  }

  async function seed() {
    if (!proposal || thinking) return;
    error = null;
    thinking = true;
    designBusy.set(true);
    try {
      seedReport = await api.designSeedBacklog(proposal.plan);
      proposal = null;
    } catch (e) {
      error = String(e);
    } finally {
      thinking = false;
      designBusy.set(false);
    }
  }

  function onKey(e: KeyboardEvent) {
    // With options present and an empty draft, Up/Down move the highlight and Enter picks the
    // highlighted option. Once the user types, Enter sends their own text instead.
    if (options.length > 0 && draft.trim() === "") {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        optionIndex = (optionIndex + 1) % options.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        optionIndex = (optionIndex - 1 + options.length) % options.length;
        return;
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        // Guard against a stale index if options ever shrink without a reset.
        void sendAnswer(options[optionIndex] ?? options[0] ?? "");
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      reply();
    }
  }

  let ratifying = $derived(step?.kind === "ratify");
  let questioning = $derived(step?.kind === "question");
  // The options offered with the current question (empty for an open question / non-question).
  let options = $derived(step?.kind === "question" ? step.options : []);
  let optionIndex = $state(0);
</script>

<div class="pane">
  {#if status === null}
    <div class="picker">
      <h2>Design a project</h2>
      <p>Pick the shape of what you are building. The chain adapts its movements to it.</p>
      {#if startOverShape}
        <div class="start-over-confirm">
          <p>
            This project already has a design conversation. Starting over replaces it with a fresh
            one. A backup of the current session is kept on disk (under <code>.tutti/design</code>).
            Continue?
          </p>
          <div class="start-over-actions">
            <button class="accent" onclick={startConfirmed} disabled={thinking}>
              Start over
            </button>
            <button class="ghost" onclick={() => (startOverShape = null)} disabled={thinking}>
              Cancel
            </button>
          </div>
        </div>
      {:else}
        <div class="shapes">
          {#each SHAPES as s (s.id)}
            <button class="shape" onclick={() => start(s.id)} disabled={thinking}>{s.label}</button>
          {/each}
        </div>
      {/if}
      {#if error}
        <div class="chat-error">{error}</div>
      {/if}
    </div>
  {:else}
    <div class="body">
      <div class="chat" style="width:{split.width}px">
        <div class="progress">
          {#each status.movements as m (m)}
            <span
              class="pip"
              class:done={status.ratified.includes(m)}
              class:current={status.current === m}
              title={movementLabel(m)}>{movementLabel(m)}</span
            >
          {/each}
        </div>

        <div class="transcript" bind:this={transcriptEl} onscroll={onTranscriptScroll}>
          {#each messages as m, i (i)}
            <!-- The in-flight agent "text" bubble accumulates raw streamed tokens, which for a
                 question turn is a `{"ask": "..."}` JSON object. Showing it flashes the JSON
                 before the finalized question replaces it, so hide it and let the "Thinking..."
                 indicator stand in until the clean question or section lands. -->
            {#if !(m.role === "agent" && m.kind === "text")}
              <div class="msg {m.role} {m.kind}">
                {#if m.kind === "section"}
                  <!-- A proposed artifact is markdown; render it (sanitized) so headings, bold,
                       and lists format instead of showing raw `##`/`**` in a monospace block. -->
                  <div class="bubble md" use:externalLinks>{@html renderMarkdown(m.text)}</div>
                {:else}
                  <div class="bubble">{m.text}</div>
                {/if}
              </div>
            {/if}
          {/each}
          {#if thinking}
            <div class="msg agent"><div class="bubble thinking">Thinking...</div></div>
          {/if}
        </div>

        {#if error}
          <div class="chat-error">{error}</div>
        {/if}

        {#if questioning}
          {#if options.length > 0}
            <!-- Selectable suggested answers. Click one to send it; or use Up/Down to highlight
                 and Enter to pick (handled in onKey). Typing in the box sends a custom answer.
                 Keyed by index (options are ephemeral and a model may repeat a string). -->
            <div class="options">
              {#each options as opt, i (i)}
                <button
                  type="button"
                  class="option"
                  class:highlighted={i === optionIndex && draft.trim() === ""}
                  onclick={() => sendAnswer(opt)}
                  disabled={thinking}>{opt}</button
                >
              {/each}
            </div>
          {/if}
          <div class="compose">
            <textarea
              bind:this={composerEl}
              bind:value={draft}
              onkeydown={onKey}
              use:autogrow={{ value: draft }}
              placeholder={options.length > 0
                ? "Pick an option above, or type your own answer..."
                : "Answer the question..."}
              disabled={thinking}></textarea>
            <button onclick={reply} disabled={thinking || !draft.trim()}>Send</button>
          </div>
        {:else if ratifying}
          <div class="ratify-bar">
            {#if reviseOpen}
              <textarea
                bind:value={reviseText}
                use:autogrow={{ value: reviseText }}
                placeholder="What should change?"
                disabled={thinking}></textarea>
              <div class="ratify-actions">
                <button onclick={revise} disabled={thinking || !reviseText.trim()}
                  >Send changes</button
                >
                <button class="ghost" onclick={() => (reviseOpen = false)} disabled={thinking}>
                  Back
                </button>
              </div>
            {:else}
              <div class="ratify-actions">
                <button class="accent" onclick={ratify} disabled={thinking}>Ratify section</button>
                <button class="ghost" onclick={() => (reviseOpen = true)} disabled={thinking}>
                  Request changes
                </button>
              </div>
            {/if}
          </div>
        {:else if proposal}
          <div class="backlog">
            <div class="backlog-title">Proposed backlog</div>
            <div class="plan">
              {#if proposal.plan.milestone}
                <div class="plan-milestone">
                  {proposal.plan.milestone.title}{proposal.plan.milestone.due
                    ? ` (due ${proposal.plan.milestone.due})`
                    : ""}
                </div>
              {/if}
              <!-- Keyed by index: the plan is ephemeral (re-rendered on each propose) and a model
                   may emit two epics or issues with the same title, which a title key would throw
                   on (each_key_duplicate). -->
              {#each proposal.plan.epics ?? [] as epic, ei (ei)}
                <div class="plan-epic">
                  <div class="plan-epic-title">{epic.title}</div>
                  <ul class="plan-issues">
                    {#each epic.issues as issue, ii (ii)}
                      <li>
                        {issue.title}
                        {#if issue.acceptance && issue.acceptance.length}
                          <span class="plan-meta">{issue.acceptance.length} AC</span>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                </div>
              {/each}
              {#if proposal.plan.loose_issues && proposal.plan.loose_issues.length}
                <div class="plan-epic">
                  <div class="plan-epic-title">Loose issues</div>
                  <ul class="plan-issues">
                    {#each proposal.plan.loose_issues as issue, li (li)}
                      <li>
                        {issue.title}
                        {#if issue.acceptance && issue.acceptance.length}
                          <span class="plan-meta">{issue.acceptance.length} AC</span>
                        {/if}
                      </li>
                    {/each}
                  </ul>
                </div>
              {/if}
            </div>
            {#if proposal.scaffold_pending && !scaffolded}
              <div class="scaffold-step">
                <div class="scaffold-title">Scaffold the stack</div>
                {#if recommendedStack}
                  <p class="scaffold-hint">
                    Tutti recommends <strong>{stackLabel(recommendedStack)}</strong>
                    from the design{proposal.plan.stack_rationale
                      ? `: ${proposal.plan.stack_rationale.replace(/[.!?]?$/, ".")}`
                      : "."} Change it below if you prefer another. Tutti lays down its lint, type, test,
                    gate, and CI setup, then seeds the backlog.
                  </p>
                {:else}
                  <p class="scaffold-hint">
                    You deferred the stack to the design chat. Pick the one the conversation landed
                    on. Tutti lays down its lint, type, test, gate, and CI setup, then seeds the
                    backlog.
                  </p>
                {/if}
                <label class="scaffold-label" for="scaffold-stack">Stack</label>
                <select id="scaffold-stack" bind:value={scaffoldStack} disabled={thinking}>
                  {#each DESIGN_STACKS as s (s.id)}
                    <option value={s.id}>{s.label}</option>
                  {/each}
                </select>
                {#if scaffoldReport && !scaffoldReport.pushed}
                  <div class="scaffold-report scaffold-warn">
                    Scaffolded {scaffoldReport.stack} ({scaffoldReport.written} file(s) written), but
                    it was not committed or pushed:
                    <ul>
                      {#each scaffoldReport.warnings as w (w)}
                        <li>{w}</li>
                      {/each}
                    </ul>
                    Fix the cause and retry, or seed the backlog without publishing the scaffold.
                  </div>
                {:else if scaffoldReport && scaffoldReport.warnings.length > 0}
                  <div class="scaffold-report scaffold-warn">
                    Scaffold warnings:
                    <ul>
                      {#each scaffoldReport.warnings as w (w)}
                        <li>{w}</li>
                      {/each}
                    </ul>
                  </div>
                {/if}
                <div class="ratify-actions">
                  <button class="accent" onclick={scaffold} disabled={thinking}>
                    {scaffoldReport && !scaffoldReport.pushed
                      ? "Retry scaffold"
                      : "Scaffold and continue"}
                  </button>
                  {#if scaffoldReport && !scaffoldReport.pushed}
                    <button class="ghost" onclick={() => (scaffolded = true)} disabled={thinking}>
                      Seed anyway
                    </button>
                  {/if}
                  <button class="ghost" onclick={() => (proposal = null)} disabled={thinking}>
                    Cancel
                  </button>
                </div>
              </div>
            {:else}
              {#if scaffoldReport}
                <div class="scaffold-report">
                  Scaffolded {scaffoldReport.stack} ({scaffoldReport.written} file(s) written,
                  {scaffoldReport.skipped} skipped).
                  {#if scaffoldReport.warnings.length > 0}
                    <ul>
                      {#each scaffoldReport.warnings as w (w)}
                        <li>{w}</li>
                      {/each}
                    </ul>
                  {/if}
                </div>
              {/if}
              <div class="ratify-actions">
                <button class="accent" onclick={seed} disabled={thinking}>Seed backlog</button>
                <button class="ghost" onclick={() => (proposal = null)} disabled={thinking}>
                  Cancel
                </button>
              </div>
            {/if}
          </div>
        {:else if seedReport}
          <div class="seed-report">
            Seeded {seedReport.created.length} issue(s), skipped {seedReport.skipped.length} already present.
          </div>
        {:else if status.complete}
          <div class="compose-actions">
            <button class="accent" onclick={propose} disabled={thinking}>Propose backlog</button>
          </div>
        {:else if !thinking}
          <div class="compose-actions">
            <button class="accent" onclick={begin}>
              {status.ratified.length === 0 ? "Start" : "Continue"}
              {status.current ? movementLabel(status.current) : ""}
            </button>
          </div>
        {/if}
      </div>

      <Resizer onResize={split.onResize} ariaLabel="Resize the chat and preview panes" />

      <div class="preview">
        {#if preview}
          <iframe class="preview-frame" title="Design preview" sandbox="" srcdoc={preview}></iframe>
        {:else}
          <div class="preview-empty">The design page appears here as sections are ratified.</div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .picker {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 24px;
    text-align: center;
    color: var(--text);
  }
  .picker p {
    color: var(--text-dim);
    font-size: 13px;
    max-width: 420px;
  }
  .shapes {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    justify-content: center;
  }
  .shape {
    padding: 10px 16px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
    font: inherit;
    font-size: 13px;
  }
  .shape:hover {
    background: var(--hover);
  }
  .start-over-confirm {
    max-width: 420px;
    text-align: center;
  }
  .start-over-confirm code {
    font-size: 12px;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--bg-panel);
  }
  .start-over-actions {
    display: flex;
    gap: 8px;
    justify-content: center;
    margin-top: 10px;
  }
  .start-over-actions button {
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
    font: inherit;
    font-size: 13px;
  }
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .chat {
    /* Width is set inline (draggable, persisted); the preview flexes to fill the rest. The
       max-width caps a large persisted/dragged width against the viewport so the preview and the
       drag handle can never be pushed off-screen on a narrow window. */
    flex: none;
    max-width: 75vw;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-right: 1px solid var(--border);
  }
  .progress {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
  }
  .pip {
    font-size: 10px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--text-faint);
  }
  .pip.done {
    color: var(--on-accent);
    background: var(--done);
    border-color: var(--done);
  }
  .pip.current {
    color: var(--text);
    border-color: var(--accent);
  }
  .transcript {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .msg {
    display: flex;
  }
  .msg.user {
    justify-content: flex-end;
  }
  .bubble {
    max-width: 80%;
    padding: 8px 12px;
    border-radius: 10px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    white-space: pre-wrap;
    font-size: 13px;
    line-height: 1.5;
  }
  .msg.user .bubble {
    background: var(--accent-bg);
    border-color: var(--accent-border);
  }
  /* A proposed section is a wider, rendered-markdown block rather than a chat bubble. */
  .msg.agent.section .bubble.md {
    max-width: 100%;
    width: 100%;
    /* A wide markdown table scrolls horizontally instead of squishing its columns. */
    overflow-x: auto;
  }
  .bubble.md :global(table) {
    border-collapse: collapse;
    margin: 8px 0;
    font-size: 12px;
  }
  .bubble.md :global(th),
  .bubble.md :global(td) {
    border: 1px solid var(--border);
    padding: 4px 8px;
    text-align: left;
    vertical-align: top;
  }
  .bubble.md :global(th) {
    background: var(--bg);
    font-weight: 600;
  }
  .bubble.md :global(blockquote) {
    margin: 6px 0;
    padding-left: 10px;
    border-left: 2px solid var(--border);
    color: var(--text-dim);
  }
  .bubble.md :global(h1),
  .bubble.md :global(h2),
  .bubble.md :global(h3) {
    font-size: 15px;
    margin: 10px 0 6px;
  }
  .bubble.md :global(h1:first-child),
  .bubble.md :global(h2:first-child),
  .bubble.md :global(h3:first-child) {
    margin-top: 0;
  }
  .bubble.md :global(p) {
    margin: 4px 0;
  }
  .bubble.md :global(ul),
  .bubble.md :global(ol) {
    margin: 4px 0;
    padding-left: 20px;
  }
  .bubble.md :global(li) {
    margin: 1px 0;
  }
  /* marked wraps loose-list items (blank line between them) in <p>, whose margins otherwise
     stack with the <li> margin into large gaps. Collapse the inner paragraph margins. */
  .bubble.md :global(li) :global(p) {
    margin: 0;
  }
  /* Trim the leading/trailing margin of the first/last block so the bubble is not top/bottom
     padded twice (its own padding plus a block margin). */
  .bubble.md :global(> :first-child) {
    margin-top: 0;
  }
  .bubble.md :global(> :last-child) {
    margin-bottom: 0;
  }
  .bubble.md :global(code) {
    font-size: 12px;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--bg);
  }
  .bubble.md :global(pre) {
    overflow-x: auto;
    padding: 8px 10px;
    border-radius: 6px;
    background: var(--bg);
  }
  .bubble.md :global(a) {
    color: var(--accent);
  }
  .bubble.thinking {
    color: var(--text-faint);
    font-style: italic;
  }
  .chat-error {
    font-size: 11px;
    padding: 6px 14px;
    background: rgba(239, 68, 68, 0.12);
    color: #ef4444;
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px 16px 0;
  }
  .option {
    text-align: left;
    padding: 8px 12px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
    font: inherit;
    font-size: 13px;
  }
  .option.highlighted {
    border-color: var(--accent);
    background: var(--accent-bg);
  }
  .option:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .compose {
    display: flex;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border);
  }
  .compose textarea,
  .ratify-bar textarea {
    flex: 1;
    /* Height is driven by the autogrow action (grows to fit the message up to a cap). */
    resize: none;
    min-height: 44px;
    padding: 8px 10px;
    font: inherit;
    font-size: 13px;
    background: var(--bg-panel);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .compose button {
    align-self: flex-end;
    height: 44px;
    padding: 8px 16px;
    cursor: pointer;
  }
  .compose-actions,
  .ratify-bar,
  .backlog,
  .seed-report {
    padding: 12px 16px;
    border-top: 1px solid var(--border);
  }
  .ratify-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
  .ratify-actions button,
  .compose-actions button {
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
    font: inherit;
    font-size: 13px;
  }
  .accent {
    background: var(--accent) !important;
    color: var(--on-accent) !important;
    border: none !important;
  }
  .ghost {
    background: transparent;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .backlog-title {
    font-weight: 600;
    font-size: 13px;
    margin-bottom: 6px;
  }
  .plan {
    max-height: 300px;
    overflow: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 13px;
  }
  .plan-milestone {
    font-weight: 600;
    padding-bottom: 6px;
    margin-bottom: 6px;
    border-bottom: 1px solid var(--border);
  }
  .plan-epic {
    margin: 8px 0;
  }
  .plan-epic-title {
    font-weight: 600;
    margin-bottom: 2px;
  }
  .plan-issues {
    margin: 0;
    padding-left: 18px;
  }
  .plan-issues li {
    margin: 2px 0;
  }
  .plan-meta {
    color: var(--text-faint);
    font-size: 11px;
    margin-left: 4px;
  }
  .seed-report {
    font-size: 13px;
    color: var(--text-dim);
  }
  .scaffold-step {
    margin-top: 12px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }
  .scaffold-title {
    font-weight: 600;
    font-size: 13px;
    margin-bottom: 4px;
  }
  .scaffold-hint {
    font-size: 12px;
    color: var(--text-dim);
    margin: 0 0 8px;
  }
  .scaffold-label {
    display: block;
    font-size: 12px;
    color: var(--text-dim);
    margin-bottom: 4px;
  }
  .scaffold-step select {
    width: 100%;
    padding: 8px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
  .scaffold-report {
    font-size: 13px;
    color: var(--text-dim);
    margin-bottom: 8px;
  }
  .scaffold-warn {
    color: var(--danger, #ff8c6b);
  }
  .scaffold-report ul {
    margin: 4px 0 0;
    padding-left: 18px;
  }
  .preview {
    flex: 1;
    min-width: 0;
    display: flex;
  }
  .preview-frame {
    flex: 1;
    border: none;
    background: white;
  }
  .preview-empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    font-size: 13px;
    padding: 24px;
    text-align: center;
  }
</style>
