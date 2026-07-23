<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Browse a forge you are already authenticated to, pick a namespace, pick a repo, and
     clone it locally. Mirrors InitWizard's modal shell (scrim, header with step dots,
     Back/Next footer, Escape to cancel, Tab focus-trap, focus-first-control on step
     change). The namespace and repo lists load on entry, each guarded by a generation
     token so a slow response for an earlier selection cannot paint over the current one. -->
<script lang="ts">
  import { api } from "$lib/ipc";
  import type { Namespace, RemoteRepo } from "$lib/ipc";
  import { browseSteps, filterRepos, cloneTarget, validateBrowseStep } from "$lib/browse";
  import QuestionCard from "./QuestionCard.svelte";

  let {
    onCancel,
    onCloned,
  }: {
    onCancel: () => void;
    // Called with the local path once a repo is cloned (or an existing checkout reused).
    onCloned: (dir: string) => Promise<void>;
  } = $props();

  let forgeKind = $state("github");
  let login = $state("");
  let step = $state(0);

  let namespaces = $state<Namespace[]>([]);
  let nsLoading = $state(false);
  let nsError = $state<string | null>(null);
  let nsQuery = $state("");
  let selectedNs = $state<Namespace | null>(null);

  let repos = $state<RemoteRepo[]>([]);
  let reposLoading = $state(false);
  let reposError = $state<string | null>(null);
  let repoQuery = $state("");
  let selectedRepo = $state<RemoteRepo | null>(null);

  let parentDir = $state("");
  let cloning = $state(false);
  let cloneError = $state<string | null>(null);

  let modalEl = $state<HTMLElement | null>(null);
  let bodyEl = $state<HTMLElement | null>(null);

  const steps = browseSteps();
  let current = $derived(steps[Math.min(step, steps.length - 1)]);
  let last = $derived(step >= steps.length - 1);
  let forgeError = $derived(validateBrowseStep({ forgeKind, login }, "forge"));

  // The CLI a given forge drives, so an auth failure can tell the user exactly which
  // tool to log into.
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
  let filteredRepos = $derived(filterRepos(repos, repoQuery));
  let target = $derived(
    selectedRepo && parentDir ? cloneTarget(parentDir, selectedRepo.name) : null,
  );

  // Whether Next may fire for the current step. The forge step defers to the shared
  // validator; the list steps need a selection (picking a row also advances, but Back
  // then Next must still work); the destination step has its own Clone button.
  let canAdvance = $derived.by(() => {
    if (current === "forge") return forgeError === null;
    if (current === "namespace") return selectedNs !== null;
    if (current === "repo") return selectedRepo !== null;
    return false;
  });

  // Load the namespaces on entering the namespace step. The generation token drops a
  // response that lands after a newer request was issued, keyed here on the forge and
  // login, so switching forge before a slow list returns cannot show stale namespaces.
  let nsSeq = 0;
  $effect(() => {
    if (current !== "namespace") return;
    // Track the inputs the list depends on so a change re-runs the load.
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

  // Load the repos on entering the repo step, keyed on the chosen namespace. Same
  // generation-token guard so a slow list for a previous namespace cannot overwrite the
  // current one.
  let repoSeq = 0;
  $effect(() => {
    if (current !== "repo") return;
    const ns = selectedNs;
    if (!ns) return;
    const kind = forgeKind;
    const who = login.trim() || null;
    repos = [];
    reposError = null;
    reposLoading = true;
    selectedRepo = null;
    repoSeq += 1;
    const seq = repoSeq;
    api
      .listRepos(kind, who, ns)
      .then((rs) => {
        if (seq === repoSeq) repos = rs;
      })
      .catch((e) => {
        if (seq === repoSeq) reposError = String(e);
      })
      .finally(() => {
        if (seq === repoSeq) reposLoading = false;
      });
  });

  // Move focus to the step's first control on every step change, so the modal owns focus
  // from the moment it opens and a keyboard user is not left tabbing from the top of the
  // document into the shell behind the scrim.
  $effect(() => {
    // Track the step so this re-runs on navigation.
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
    selectedRepo = null;
    repoQuery = "";
    step += 1;
  }

  function pickRepo(r: RemoteRepo) {
    selectedRepo = r;
    step += 1;
  }

  async function chooseParent() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true });
    if (typeof picked === "string") {
      parentDir = picked;
      cloneError = null;
    }
  }

  async function clone() {
    if (cloning || !selectedRepo || !parentDir) return;
    cloning = true;
    cloneError = null;
    try {
      const path = await api.cloneRepo(selectedRepo.clone_url, parentDir, selectedRepo.name);
      await onCloned(path);
    } catch (e) {
      cloneError = String(e);
    } finally {
      cloning = false;
    }
  }

  const FOCUSABLE = 'input, select, textarea, button:not([tabindex="-1"]), [href]';

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
      return;
    }
    // Keep Tab inside the modal. Without this, tabbing past the last control walks into
    // the shell behind the scrim, which is inert but still focusable.
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
    // Enter advances, but never when it would steal a focused button's own activation,
    // and never mid-composition (an IME commit also fires Enter).
    if (e.key === "Enter" && !e.isComposing && !(e.target instanceof HTMLButtonElement)) {
      if (e.target instanceof HTMLTextAreaElement) return;
      e.preventDefault();
      if (last) void clone();
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
  aria-label="Add a project from a forge"
  bind:this={modalEl}
>
  <header>
    <span class="title">Add a project from a forge</span>
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
        question="Which forge do you want to browse?"
        description="Tutti lists the repos you can reach through your forge's command-line tool, using the login you already have."
        error={forgeError}
      >
        <label class="opt" class:on={forgeKind === "github"}>
          <input type="radio" bind:group={forgeKind} value="github" />
          <span
            ><b>GitHub</b><em
              >Your repos and organizations on github.com or GitHub Enterprise. Requires the `gh`
              CLI, already logged in.</em
            ></span
          >
        </label>
        <label class="opt" class:on={forgeKind === "gitlab"}>
          <input type="radio" bind:group={forgeKind} value="gitlab" />
          <span
            ><b>GitLab</b><em
              >Your projects and groups on gitlab.com or a self-hosted instance. Requires the `glab`
              CLI, already logged in.</em
            ></span
          >
        </label>
        <label class="opt" class:on={forgeKind === "gitea"}>
          <input type="radio" bind:group={forgeKind} value="gitea" />
          <span
            ><b>Gitea</b><em
              >Your repos and orgs on Gitea, Forgejo or Codeberg. Requires the `tea` CLI, already
              logged in.</em
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
        question="Which account or organization?"
        description="Where the repo lives: your own account, or one of the orgs or groups you belong to."
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
    {:else if current === "repo"}
      <QuestionCard
        question="Which repository?"
        description="The repo Tutti will clone and adopt. It records exactly this repo in the project's tutti.toml."
      >
        {#if reposLoading}
          <div class="loading">Loading repositories...</div>
        {:else if reposError}
          <div class="err-block">
            <div>
              Could not list repositories. Check that the `{cliName(forgeKind)}` CLI is installed
              and authenticated.
            </div>
            <div class="err-detail">{reposError}</div>
          </div>
        {:else}
          <input
            class="search"
            bind:value={repoQuery}
            placeholder="Search repositories..."
            aria-label="Search repositories"
          />
          {#if filteredRepos.length === 0}
            <div class="empty">No repositories match.</div>
          {:else}
            <div class="list" role="list">
              {#each filteredRepos as r (r.full_path)}
                <button
                  type="button"
                  class="row repo-row"
                  class:on={selectedRepo?.full_path === r.full_path}
                  onclick={() => pickRepo(r)}
                >
                  <span class="repo-head">
                    <span class="row-name">{r.name}</span>
                    {#if r.private}<span class="badge">private</span>{/if}
                    {#if r.archived}<span class="badge muted">archived</span>{/if}
                  </span>
                  {#if r.description}
                    <span class="repo-desc">{r.description}</span>
                  {/if}
                </button>
              {/each}
            </div>
          {/if}
        {/if}
      </QuestionCard>
    {:else}
      <QuestionCard
        question="Where should it be cloned?"
        description="Pick the folder to clone into. The repo lands in a subfolder named after it. If a matching git checkout is already there, Tutti reuses it."
        error={cloneError}
      >
        <button type="button" class="ghost" onclick={chooseParent}>Choose parent folder...</button>
        {#if selectedRepo}
          <div class="target-label">Clone target</div>
          {#if target}
            <div class="path">{target}</div>
          {:else}
            <div class="empty">Choose a parent folder to see where {selectedRepo.name} lands.</div>
          {/if}
        {/if}
      </QuestionCard>
    {/if}
  </div>

  <footer>
    <button type="button" class="ghost" onclick={onCancel}>Cancel</button>
    <div class="spacer"></div>
    <button type="button" class="ghost" disabled={step === 0} onclick={back}>Back</button>
    {#if last}
      <button type="button" class="primary" disabled={cloning || !target} onclick={clone}>
        {cloning ? "Cloning..." : "Clone"}
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
  .sub-label {
    font-size: 12px;
    font-weight: 620;
  }
  .sub-desc {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-dim);
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
  .repo-row {
    flex-direction: column;
    align-items: stretch;
    gap: 4px;
  }
  .repo-head {
    display: flex;
    align-items: center;
    gap: 8px;
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
  .repo-desc {
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-dim);
  }
  .badge {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid var(--accent-border);
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .badge.muted {
    border-color: var(--border);
    color: var(--text-faint);
  }
  .target-label {
    margin-top: 6px;
    font-size: 12px;
    font-weight: 620;
  }
  input:not([type="radio"]) {
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
