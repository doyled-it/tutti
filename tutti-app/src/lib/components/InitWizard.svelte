<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The guided new-project wizard: one question per screen, each explaining what the
     choice does, offering the legal options, and showing an example. The last step
     previews the exact tutti.toml the backend will write. -->
<script lang="ts">
  import { untrack } from "svelte";
  import { api } from "$lib/ipc";
  import type { InitForm, Probe } from "$lib/ipc";
  import {
    initialState,
    validateStep,
    toInitForm,
    STEP_COUNT,
    KNOWN_MODELS,
    type WizardState,
  } from "$lib/wizard";
  import QuestionCard from "./QuestionCard.svelte";

  let {
    dir,
    probe,
    onCancel,
    onCreate,
    onRepick,
  }: {
    dir: string;
    probe: Probe;
    onCancel: () => void;
    onCreate: (form: InitForm) => Promise<void>;
    onRepick: () => void;
  } = $props();

  // Seeded once from the props here, and re-seeded by the effect below when a repick
  // swaps the target folder underneath a mounted wizard. untrack makes the one-shot
  // capture explicit rather than an accidental snapshot.
  let s = $state<WizardState>(untrack(() => initialState(dir, probe)));
  let seededDir = untrack(() => dir);
  let step = $state(0);
  let submitting = $state(false);
  let submitError = $state<string | null>(null);
  let preview = $state<string | null>(null);
  let previewError = $state<string | null>(null);
  let detectedRepo = $derived(probe.repo);

  // "Choose a different folder..." replaces the wizard's target in place, so re-seed
  // from the new folder and its probe. That button only exists on step 0, so there are
  // no answers to lose.
  $effect(() => {
    if (dir === seededDir) return;
    seededDir = dir;
    s = initialState(dir, probe);
    step = 0;
  });

  let error = $derived(validateStep(s, step));
  let last = $derived(step === STEP_COUNT - 1);

  const STEPS = Array.from({ length: STEP_COUNT }, (_, i) => i);

  const REPO_EXAMPLE: Record<string, string> = {
    github: "doyled-it/oxidra",
    gitea: "doyled-it/oxidra",
    gitlab: "group/subgroup/project",
  };

  function next() {
    if (error || last) return;
    step += 1;
  }

  function back() {
    if (step > 0) step -= 1;
  }

  // Fetch the rendered file whenever the review step is reached, so it always reflects
  // the answers as they stand rather than a stale render from an earlier visit.
  $effect(() => {
    if (step !== STEP_COUNT - 1) return;
    preview = null;
    previewError = null;
    const form = toInitForm(s);
    api
      .previewTuttiToml(form)
      .then((text) => (preview = text))
      .catch((e) => (previewError = String(e)));
  });

  async function create() {
    if (submitting) return;
    submitting = true;
    submitError = null;
    try {
      await onCreate(toInitForm(s));
    } catch (e) {
      submitError = String(e);
    } finally {
      submitting = false;
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
      return;
    }
    // Enter advances, except inside a textarea and except when the step is invalid.
    if (e.key === "Enter" && !(e.target instanceof HTMLTextAreaElement)) {
      e.preventDefault();
      if (last) void create();
      else next();
    }
  }

  function setGate(i: number, v: string) {
    s.gateCommands[i] = v;
  }
  function addGate() {
    s.gateCommands = [...s.gateCommands, ""];
  }
  function removeGate(i: number) {
    s.gateCommands = s.gateCommands.filter((_, j) => j !== i);
  }
  function setSkip(i: number, v: string) {
    s.skipLabels[i] = v;
  }
  function addSkip() {
    s.skipLabels = [...s.skipLabels, ""];
  }
  function removeSkip(i: number) {
    s.skipLabels = s.skipLabels.filter((_, j) => j !== i);
  }
</script>

<svelte:window onkeydown={onKeydown} />

<button type="button" class="scrim" aria-label="Cancel" tabindex="-1" onclick={onCancel}></button>

<div class="modal" role="dialog" aria-modal="true" aria-label="New Tutti project">
  <header>
    <span class="title">New Tutti project</span>
    <span class="counter">Step {step + 1} of {STEP_COUNT}</span>
    <div class="dots">
      {#each STEPS as i (i)}
        <span class="dot" class:on={i === step} class:past={i < step}></span>
      {/each}
    </div>
  </header>

  <div class="body">
    {#if step === 0}
      <QuestionCard
        question="Which folder?"
        description="The local git checkout Tutti will work in. It must already be a git repo with your project in it."
        note={detectedRepo
          ? `Detected the repo ${detectedRepo} from this folder's git remote.`
          : "No git remote detected here, so you will need to type the repo yourself on the next steps."}
      >
        <div class="path">{s.dir}</div>
        <button type="button" class="ghost" onclick={onRepick}>Choose a different folder...</button>
      </QuestionCard>
    {:else if step === 1}
      <QuestionCard
        question="Which forge hosts this project?"
        description="Tutti reads issues and opens pull requests through your forge's command-line tool, using the login you already have."
        {error}
      >
        <label class="opt" class:on={s.forgeKind === "github"}>
          <input type="radio" bind:group={s.forgeKind} value="github" />
          <span
            ><b>GitHub</b><em
              >Issues and pull requests on github.com or GitHub Enterprise. Requires the `gh` CLI,
              already logged in.</em
            ></span
          >
        </label>
        <label class="opt" class:on={s.forgeKind === "gitlab"}>
          <input type="radio" bind:group={s.forgeKind} value="gitlab" />
          <span
            ><b>GitLab</b><em
              >Issues, merge requests and epics on gitlab.com or a self-hosted instance. Requires
              the `glab` CLI, already logged in.</em
            ></span
          >
        </label>
        <label class="opt" class:on={s.forgeKind === "gitea"}>
          <input type="radio" bind:group={s.forgeKind} value="gitea" />
          <span
            ><b>Gitea</b><em
              >Issues and pull requests on Gitea, Forgejo or Codeberg. Requires the `tea` CLI,
              already logged in.</em
            ></span
          >
        </label>
        {#if s.forgeKind === "gitea"}
          <div class="sub">
            <label class="sub-label" for="tea-login">Which tea login?</label>
            <div class="sub-desc">
              The name you gave the host when you ran `tea login add`. Run `tea login list` to see
              yours.
            </div>
            <input id="tea-login" bind:value={s.login} placeholder="codeberg" />
          </div>
        {/if}
      </QuestionCard>
    {:else if step === 2}
      <QuestionCard
        question="Which repository?"
        description="The repo Tutti reads issues from and opens pull requests against."
        example={REPO_EXAMPLE[s.forgeKind] ?? "owner/repo"}
        note={detectedRepo ? "Detected from your git remote." : null}
        {error}
      >
        <input bind:value={s.repo} placeholder={REPO_EXAMPLE[s.forgeKind] ?? "owner/repo"} />
      </QuestionCard>
    {:else if step === 3}
      <QuestionCard
        question="What is your trunk branch?"
        description="Your protected branch. Tutti never merges into it and never commits to it directly. Promoting finished work from the integration branch up to trunk stays a human decision."
        fallback="main"
        {error}
      >
        <input bind:value={s.trunk} placeholder="main" />
      </QuestionCard>
    {:else if step === 4}
      <QuestionCard
        question="How should work be routed?"
        description="Where each issue's branch comes from, and where it merges back to."
        {error}
      >
        <label class="opt" class:on={s.routing === "trunk"}>
          <input type="radio" bind:group={s.routing} value="trunk" />
          <span
            ><b>Trunk (recommended)</b><em
              >Every issue branches off one integration branch and merges back into it. Simple, and
              what you want unless you are running phased milestones.</em
            ></span
          >
        </label>
        <label class="opt" class:on={s.routing === "phase_stacking"}>
          <input type="radio" bind:group={s.routing} value="phase_stacking" />
          <span
            ><b>Phase stacking</b><em
              >Each milestone gets its own integration branch, stacked on the previous one, so phase
              N builds on phase N-1 before any of it reaches trunk.</em
            ></span
          >
        </label>
        {#if s.routing === "trunk"}
          <div class="sub">
            <label class="sub-label" for="integration-branch">Which integration branch?</label>
            <div class="sub-desc">
              The branch Tutti merges finished work into. It must exist, and must not be your trunk
              branch. Default: staging.
            </div>
            <input id="integration-branch" bind:value={s.integrationBranch} placeholder="staging" />
          </div>
        {/if}
      </QuestionCard>
    {:else if step === 5}
      <QuestionCard
        question="Which model should the agent run as?"
        description="Sonnet is the balanced default. Opus is stronger and slower on hard work. Haiku is fastest and cheapest for mechanical tasks."
        fallback="claude-sonnet-5"
        {error}
      >
        <select
          aria-label="Model"
          value={s.modelCustom ? "__custom" : s.model}
          onchange={(e) => {
            const v = e.currentTarget.value;
            if (v === "__custom") {
              s.modelCustom = true;
            } else {
              s.modelCustom = false;
              s.model = v;
            }
          }}
        >
          {#each KNOWN_MODELS as m (m)}
            <option value={m}>{m}</option>
          {/each}
          <option value="__custom">Custom...</option>
        </select>
        {#if s.modelCustom}
          <input bind:value={s.model} placeholder="model id" aria-label="Custom model id" />
        {/if}
      </QuestionCard>
    {:else if step === 6}
      <QuestionCard
        question="What must pass before shipping?"
        description="Commands that must pass before Tutti will ship an issue's work. They run in order in your repo root, and the first non-zero exit fails the gate and sends the work back for a fix. Leave the single `true` if you do not want a gate yet."
        example="cargo test, npm test, uv run pytest"
        {error}
      >
        {#each s.gateCommands as cmd, i (i)}
          <div class="row">
            <input
              value={cmd}
              aria-label={`Gate command ${i + 1}`}
              oninput={(e) => setGate(i, e.currentTarget.value)}
            />
            <button
              type="button"
              class="ghost small"
              aria-label="Remove command"
              onclick={() => removeGate(i)}>&times;</button
            >
          </div>
        {/each}
        <button type="button" class="ghost small self-start" onclick={addGate}>+ Add command</button
        >
      </QuestionCard>
    {:else if step === 7}
      <QuestionCard
        question="Which issues should Tutti pick up?"
        description="Tutti only picks up issues carrying the required label, and never picks up one carrying a skip label."
        {error}
      >
        <label class="sub-label" for="require-label">Required label</label>
        <input id="require-label" bind:value={s.requireLabel} placeholder="status:ready" />
        <div class="sub-label">Skip labels</div>
        {#each s.skipLabels as lab, i (i)}
          <div class="row">
            <input
              value={lab}
              aria-label={`Skip label ${i + 1}`}
              oninput={(e) => setSkip(i, e.currentTarget.value)}
            />
            <button
              type="button"
              class="ghost small"
              aria-label="Remove label"
              onclick={() => removeSkip(i)}>&times;</button
            >
          </div>
        {/each}
        <button type="button" class="ghost small self-start" onclick={addSkip}>+ Add label</button>
        <div class="callout">
          Tutti will create <code>status:ready</code>, <code>status:in-progress</code> and
          <code>status:done</code> in your forge if they do not exist yet, and will move each issue between
          them as it works.
        </div>
      </QuestionCard>
    {:else if step === 8}
      <QuestionCard
        question="How many issues per run?"
        description="How many issues one Run will work through before stopping. This is a safety ceiling, not a target: the run also stops as soon as nothing is ready."
        fallback="25"
        note="Tutti always merges with a merge commit, never a squash or a rebase."
        {error}
      >
        <input
          type="number"
          min="1"
          aria-label="Issues per run"
          value={s.maxIssuesPerRun}
          oninput={(e) => (s.maxIssuesPerRun = Number(e.currentTarget.value))}
        />
      </QuestionCard>
    {:else}
      <QuestionCard
        question="Ready to create"
        description="This is exactly what will be written to tutti.toml. Nothing has been created yet."
        error={submitError}
      >
        {#if previewError}
          <div class="preview err-preview">{previewError}</div>
        {:else if preview === null}
          <div class="preview">Rendering...</div>
        {:else}
          <pre class="preview">{preview}</pre>
        {/if}
      </QuestionCard>
    {/if}
  </div>

  <footer>
    <button type="button" class="ghost" onclick={onCancel}>Cancel</button>
    <div class="spacer"></div>
    <button type="button" class="ghost" disabled={step === 0} onclick={back}>Back</button>
    {#if last}
      <button type="button" class="primary" disabled={submitting} onclick={create}>
        {submitting ? "Creating..." : "Create project"}
      </button>
    {:else}
      <button type="button" class="primary" disabled={error !== null} onclick={next}>Next</button>
    {/if}
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    width: 100%;
    height: 100%;
    background: rgba(0, 0, 0, 0.5);
    border: none;
    border-radius: 0;
    padding: 0;
    cursor: default;
    z-index: 50;
  }
  .scrim:hover {
    background: rgba(0, 0, 0, 0.5);
  }
  .modal {
    position: fixed;
    z-index: 51;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(640px, 90vw);
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.4);
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 18px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    font-weight: 650;
  }
  .counter {
    font-size: 12px;
    color: var(--text-faint);
  }
  .dots {
    margin-left: auto;
    display: flex;
    gap: 4px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--border);
  }
  .dot.past {
    background: var(--text-faint);
  }
  .dot.on {
    background: var(--accent);
  }
  .body {
    padding: 20px 18px;
    overflow-y: auto;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 18px;
    border-top: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .path {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg);
    word-break: break-all;
  }
  .opt {
    display: flex;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }
  .opt.on {
    border-color: var(--accent-border);
    background: var(--accent-bg);
  }
  .opt b {
    display: block;
    font-weight: 620;
  }
  .opt em {
    display: block;
    margin-top: 3px;
    font-style: normal;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-dim);
  }
  .sub {
    margin-top: 4px;
    padding-left: 2px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .sub-label {
    font-size: 12px;
    font-weight: 620;
  }
  .sub-desc {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-dim);
  }
  .row {
    display: flex;
    gap: 6px;
  }
  .row input {
    flex: 1;
  }
  .self-start {
    align-self: flex-start;
  }
  .callout {
    margin-top: 6px;
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--accent-bg);
    border: 1px solid var(--accent-border);
    font-size: 12px;
    line-height: 1.55;
  }
  .callout code {
    font-size: 11px;
  }
  .preview {
    margin: 0;
    max-height: 320px;
    overflow: auto;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre;
  }
  .err-preview {
    color: #ef4444;
  }
  input:not([type="radio"]),
  select {
    padding: 7px 9px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    font-size: 13px;
  }
  button {
    padding: 7px 14px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: none;
    color: var(--text);
    font-size: 13px;
    cursor: pointer;
  }
  button.small {
    padding: 5px 9px;
  }
  button:hover:not(:disabled) {
    background: var(--hover);
  }
  button.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
