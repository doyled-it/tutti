<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Create a brand-new repo on a forge you are authenticated to, then clone it and hand
     off to the wizard. Mirrors BrowseForge's modal shell. The namespace list loads on
     entry, guarded by a generation token so a slow response for an earlier forge cannot
     paint over the current one. -->
<script lang="ts">
  import { api } from "$lib/ipc";
  import type { Namespace, NewRepo } from "$lib/ipc";
  import { createSteps, validateCreateStep, validateName } from "$lib/create";
  import { cloneTarget } from "$lib/browse";
  import QuestionCard from "./QuestionCard.svelte";

  let {
    onCancel,
    onCloned,
  }: {
    onCancel: () => void;
    // Same handoff BrowseForge uses: once the new repo is created and cloned locally,
    // hand the local path plus the chosen forge and login to the page, which probes it
    // and opens the wizard.
    onCloned: (dir: string, forgeKind: string, login: string) => Promise<void>;
  } = $props();

  let forgeKind = $state("github");
  let login = $state("");
  let step = $state(0);

  let namespaces = $state<Namespace[]>([]);
  let nsLoading = $state(false);
  let nsError = $state<string | null>(null);
  let nsQuery = $state("");
  let selectedNs = $state<Namespace | null>(null);

  let name = $state("");
  let description = $state("");
  let isPrivate = $state(true);

  let parentDir = $state("");
  let creating = $state(false);
  let createError = $state<string | null>(null);

  let modalEl = $state<HTMLElement | null>(null);
  let bodyEl = $state<HTMLElement | null>(null);

  const steps = createSteps();
  let current = $derived(steps[Math.min(step, steps.length - 1)]);
  let last = $derived(step >= steps.length - 1);
  let forgeError = $derived(validateCreateStep({ forgeKind, login, name }, "forge"));
  let nameError = $derived(validateName(name));
  let target = $derived(name && parentDir ? cloneTarget(parentDir, name.trim()) : null);

  function cliName(kind: string): string {
    if (kind === "gitlab") return "glab";
    if (kind === "gitea") return "tea";
    return "gh";
  }

  let filteredNamespaces = $derived.by(() => {
    const q = nsQuery.trim().toLowerCase();
    if (!q) return namespaces;
    return namespaces.filter(
      (n) => n.name.toLowerCase().includes(q) || n.path.toLowerCase().includes(q),
    );
  });

  let canAdvance = $derived.by(() => {
    if (current === "forge") return forgeError === null;
    if (current === "namespace") return selectedNs !== null;
    if (current === "details") return nameError === null;
    return false;
  });

  // Load namespaces on entering the namespace step, guarded by a generation token keyed
  // on the forge and login (same as BrowseForge).
  let nsSeq = 0;
  $effect(() => {
    if (current !== "namespace") return;
    const kind = forgeKind;
    const who = login.trim() || null;
    namespaces = [];
    nsError = null;
    nsLoading = true;
    selectedNs = null;
    nsSeq += 1;
    const seq = nsSeq;
    api
      .listNamespaces(kind, who)
      .then((ns) => {
        if (seq === nsSeq) namespaces = ns;
      })
      .catch((e) => {
        if (seq === nsSeq) nsError = String(e);
      })
      .finally(() => {
        if (seq === nsSeq) nsLoading = false;
      });
  });

  // Move focus to the step's first control on every step change.
  $effect(() => {
    void current;
    const el = bodyEl?.querySelector<HTMLElement>(
      'input:not([type="radio"]), input[type="radio"]:checked, select, textarea, button',
    );
    el?.focus();
  });

  function next() {
    if (last || !canAdvance) return;
    step += 1;
  }

  function back() {
    if (step > 0) step -= 1;
  }

  function pickNamespace(ns: Namespace) {
    selectedNs = ns;
    step += 1;
  }

  async function chooseParent() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true });
    if (typeof picked === "string") {
      parentDir = picked;
      createError = null;
    }
  }

  async function create() {
    if (creating || !selectedNs || !parentDir || nameError !== null) return;
    creating = true;
    createError = null;
    try {
      const spec: NewRepo = {
        name: name.trim(),
        description: description.trim() ? description.trim() : null,
        private: isPrivate,
      };
      const repo = await api.createRepo(forgeKind, login.trim() || null, selectedNs, spec);
      const path = await api.cloneRepo(repo.clone_url, parentDir, repo.name);
      await onCloned(path, forgeKind, login.trim());
    } catch (e) {
      createError = String(e);
    } finally {
      creating = false;
    }
  }

  const FOCUSABLE = 'input, select, textarea, button:not([tabindex="-1"]), [href]';

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
      return;
    }
    if (e.key === "Tab" && modalEl) {
      const items = [...modalEl.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
        (el) => !el.hasAttribute("disabled"),
      );
      if (items.length === 0) return;
      const first = items[0];
      const flast = items[items.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (!active || !modalEl.contains(active)) {
        e.preventDefault();
        (e.shiftKey ? flast : first).focus();
      } else if (e.shiftKey && active === first) {
        e.preventDefault();
        flast.focus();
      } else if (!e.shiftKey && active === flast) {
        e.preventDefault();
        first.focus();
      }
      return;
    }
    if (e.key === "Enter" && !e.isComposing && !(e.target instanceof HTMLButtonElement)) {
      if (e.target instanceof HTMLTextAreaElement) return;
      e.preventDefault();
      if (last) void create();
      else next();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<button type="button" class="scrim" aria-label="Cancel" tabindex="-1" onclick={onCancel}></button>

<div
  class="modal"
  role="dialog"
  aria-modal="true"
  aria-label="Create a new repository"
  bind:this={modalEl}
>
  <header>
    <span class="title">Create a new repository</span>
    <span class="counter">Step {step + 1} of {steps.length}</span>
    <div class="dots">
      {#each steps as id, i (id)}
        <span class="dot" class:on={i === step} class:past={i < step}></span>
      {/each}
    </div>
  </header>

  <div class="body" bind:this={bodyEl}>
    {#if current === "forge"}
      <QuestionCard
        question="Which forge should host the new repo?"
        description="Tutti creates the repo through your forge's command-line tool, using the login you already have."
        error={forgeError}
      >
        <label class="opt" class:on={forgeKind === "github"}>
          <input type="radio" bind:group={forgeKind} value="github" />
          <span
            ><b>GitHub</b><em
              >Create on github.com or GitHub Enterprise. Requires the `gh` CLI, logged in.</em
            ></span
          >
        </label>
        <label class="opt" class:on={forgeKind === "gitlab"}>
          <input type="radio" bind:group={forgeKind} value="gitlab" />
          <span
            ><b>GitLab</b><em
              >Create on gitlab.com or a self-hosted instance. Requires the `glab` CLI, logged in.</em
            ></span
          >
        </label>
        <label class="opt" class:on={forgeKind === "gitea"}>
          <input type="radio" bind:group={forgeKind} value="gitea" />
          <span
            ><b>Gitea</b><em
              >Create on Gitea, Forgejo or Codeberg. Requires the `tea` CLI, logged in.</em
            ></span
          >
        </label>
        {#if forgeKind === "gitea"}
          <div class="sub">
            <label class="sub-label" for="tea-login">Which tea login?</label>
            <div class="sub-desc">
              The name you gave the host when you ran `tea login add`. Run `tea login list` to see
              yours.
            </div>
            <input id="tea-login" bind:value={login} placeholder="codeberg" />
          </div>
        {/if}
      </QuestionCard>
    {:else if current === "namespace"}
      <QuestionCard
        question="Where should it be created?"
        description="Your own account, or one of the orgs or groups you belong to."
      >
        {#if nsLoading}
          <div class="loading">Loading namespaces...</div>
        {:else if nsError}
          <div class="err-block">
            <div>
              Could not list namespaces. Check that the `{cliName(forgeKind)}` CLI is installed and
              authenticated.
            </div>
            <div class="err-detail">{nsError}</div>
          </div>
        {:else}
          <input
            class="search"
            bind:value={nsQuery}
            placeholder="Filter accounts and orgs..."
            aria-label="Filter namespaces"
          />
          {#if filteredNamespaces.length === 0}
            <div class="empty">No namespaces match.</div>
          {:else}
            <div class="list" role="list">
              {#each filteredNamespaces as ns (ns.path)}
                <button
                  type="button"
                  class="row"
                  class:on={selectedNs?.path === ns.path}
                  onclick={() => pickNamespace(ns)}
                >
                  <span class="row-name">{ns.name}</span>
                  <span class="row-kind">{ns.kind}</span>
                </button>
              {/each}
            </div>
          {/if}
        {/if}
      </QuestionCard>
    {:else if current === "details"}
      <QuestionCard
        question="Name the repository"
        description="This is exactly what Tutti records in the project's tutti.toml. It is created with a README so the clone lands cleanly."
        error={name.length > 0 ? nameError : null}
      >
        <label class="field-label" for="repo-name">Repository name</label>
        <input id="repo-name" bind:value={name} placeholder="my-project" />
        <label class="field-label" for="repo-desc">Description (optional)</label>
        <input id="repo-desc" bind:value={description} placeholder="What this project is" />
        <div class="vis">
          <label class="opt" class:on={isPrivate}>
            <input type="radio" bind:group={isPrivate} value={true} />
            <span><b>Private</b><em>Only you and people you add can see it.</em></span>
          </label>
          <label class="opt" class:on={!isPrivate}>
            <input type="radio" bind:group={isPrivate} value={false} />
            <span><b>Public</b><em>Anyone can see it.</em></span>
          </label>
        </div>
      </QuestionCard>
    {:else}
      <QuestionCard
        question="Where should it be cloned?"
        description="Pick the folder to clone the new repo into. It lands in a subfolder named after the repo."
        error={createError}
      >
        <button type="button" class="ghost" onclick={chooseParent}>Choose parent folder...</button>
        <div class="target-label">Clone target</div>
        {#if target}
          <div class="path">{target}</div>
        {:else}
          <div class="empty">
            Choose a parent folder to see where {name.trim() || "the repo"} lands.
          </div>
        {/if}
      </QuestionCard>
    {/if}
  </div>

  <footer>
    <button type="button" class="ghost" onclick={onCancel}>Cancel</button>
    <div class="spacer"></div>
    <button type="button" class="ghost" disabled={step === 0} onclick={back}>Back</button>
    {#if last}
      <button type="button" class="primary" disabled={creating || !target} onclick={create}>
        {creating ? "Creating..." : "Create & clone"}
      </button>
    {:else}
      <button type="button" class="primary" disabled={!canAdvance} onclick={next}>Next</button>
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
  .sub-label,
  .field-label {
    font-size: 12px;
    font-weight: 620;
  }
  .field-label {
    margin-top: 6px;
  }
  .sub-desc {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-dim);
  }
  .vis {
    margin-top: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .loading {
    font-size: 13px;
    color: var(--text-dim);
    padding: 6px 2px;
  }
  .empty {
    font-size: 12px;
    color: var(--text-faint);
    padding: 6px 2px;
  }
  .err-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid var(--accent-border);
    background: var(--accent-bg);
    font-size: 12px;
    line-height: 1.55;
  }
  .err-detail {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--text-dim);
    word-break: break-word;
  }
  .search {
    width: 100%;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 3px;
    max-height: 320px;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    color: var(--text);
    cursor: pointer;
  }
  .row:hover:not(:disabled) {
    background: var(--hover);
  }
  .row.on {
    border-color: var(--accent-border);
    background: var(--accent-bg);
  }
  .row-name {
    flex: 1;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-kind {
    font-size: 11px;
    color: var(--text-faint);
    text-transform: lowercase;
  }
  .target-label {
    margin-top: 6px;
    font-size: 12px;
    font-weight: 620;
  }
  input:not([type="radio"]) {
    width: 100%;
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
  .row.on:hover {
    background: var(--accent-bg);
  }
</style>
