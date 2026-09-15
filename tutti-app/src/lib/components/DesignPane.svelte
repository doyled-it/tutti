<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Design surface: a per-movement chat that drives the design chain turn by turn, a
     live preview of the accreting design page, and a backlog review/confirm step before
     seeding. Each turn is one Tauri command (begin/reply/revise/ratify); the agent's turn
     streams in over design://delta. All transcript and step logic lives in $lib/design;
     this component is wiring only. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "$lib/ipc";
  import { designBusy } from "$lib/stores";
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

  let draft = $state("");
  let reviseOpen = $state(false);
  let reviseText = $state("");
  let thinking = $state(false);
  let error = $state<string | null>(null);

  // Bound DOM nodes for the ergonomics fixes below.
  let transcriptEl = $state<HTMLDivElement | null>(null);
  let composerEl = $state<HTMLTextAreaElement | null>(null);

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
      status = await api.designStart(shape);
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

  async function reply() {
    const text = draft.trim();
    if (!text || thinking) return;
    error = null;
    messages = appendAnswer(messages, text);
    draft = "";
    step = null;
    thinking = true;
    streaming = true;
    designBusy.set(true);
    messages = startAgent(messages);
    try {
      const st = await api.designReply(text);
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
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      reply();
    }
  }

  let ratifying = $derived(step?.kind === "ratify");
  let questioning = $derived(step?.kind === "question");
</script>

<div class="pane">
  {#if status === null}
    <div class="picker">
      <h2>Design a project</h2>
      <p>Pick the shape of what you are building. The chain adapts its movements to it.</p>
      <div class="shapes">
        {#each SHAPES as s (s.id)}
          <button class="shape" onclick={() => start(s.id)} disabled={thinking}>{s.label}</button>
        {/each}
      </div>
    </div>
  {:else}
    <div class="body">
      <div class="chat">
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
                <div class="bubble">{m.text}</div>
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
          <div class="compose">
            <textarea
              bind:this={composerEl}
              bind:value={draft}
              onkeydown={onKey}
              placeholder="Answer the question..."
              disabled={thinking}></textarea>
            <button onclick={reply} disabled={thinking || !draft.trim()}>Send</button>
          </div>
        {:else if ratifying}
          <div class="ratify-bar">
            {#if reviseOpen}
              <textarea
                bind:value={reviseText}
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
            <pre class="backlog-text">{proposal.rendered}</pre>
            {#if proposal.scaffold_pending && !scaffolded}
              <div class="scaffold-step">
                <div class="scaffold-title">Scaffold the stack</div>
                <p class="scaffold-hint">
                  You deferred the stack to the design chat. Pick the one the conversation landed
                  on. Tutti lays down its lint, type, test, gate, and CI setup, then seeds the
                  backlog.
                </p>
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
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .chat {
    flex: 1;
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
  .msg.agent.section .bubble {
    font-family: monospace;
    font-size: 12px;
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
  .compose {
    display: flex;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border);
  }
  .compose textarea,
  .ratify-bar textarea {
    flex: 1;
    resize: none;
    min-height: 44px;
    max-height: 160px;
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
  .backlog-text {
    max-height: 240px;
    overflow: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px;
    font-size: 12px;
    white-space: pre-wrap;
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
