<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Left rail: the loaded project (increment 1 supports one at a time), an "open project"
     affordance, and the primary nav (Board is live; Orchestrator/Subsessions are placeholders).
     Resizable via a drag handle on the right edge, with the width persisted to localStorage. -->
<script lang="ts">
  import type { ProjectSummary } from "$lib/ipc";
  import Resizer from "./Resizer.svelte";

  let {
    project,
    onOpenProject,
  }: {
    project: ProjectSummary | null;
    onOpenProject: (dir: string, repo?: string) => Promise<void>;
  } = $props();

  const WIDTH_KEY = "tutti.sidebarWidth";
  const MIN_WIDTH = 140;
  const MAX_WIDTH = 360;
  const DEFAULT_WIDTH = 160;

  let width = $state(DEFAULT_WIDTH);

  function clamp(w: number): number {
    return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, w));
  }

  $effect(() => {
    const stored = localStorage.getItem(WIDTH_KEY);
    if (stored) {
      const parsed = Number(stored);
      if (Number.isFinite(parsed)) width = clamp(parsed);
    }
  });

  function onResize(deltaX: number) {
    width = clamp(width + deltaX);
    localStorage.setItem(WIDTH_KEY, String(width));
  }

  let adding = $state(false);
  let dir = $state("");
  let repo = $state("");
  // Only shown once auto-detect fails; the message explains why (no origin remote, or
  // an unparseable one), so the user knows why they need to type owner/repo.
  let showManual = $state(false);
  let addError = $state<string | null>(null);

  function beginAdd() {
    adding = true;
    showManual = false;
    addError = null;
  }

  function cancelAdd() {
    adding = false;
    dir = "";
    repo = "";
    showManual = false;
    addError = null;
  }

  // The common case: pick a folder and try loading with no manual repo. If the folder's
  // git origin resolves to owner/repo, the project loads immediately with no typing. If
  // there is no remote (or it can't be parsed), fall back to the manual owner/repo field.
  async function pickDir() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true });
    if (typeof picked !== "string") return;
    dir = picked;
    addError = null;
    try {
      await onOpenProject(dir);
      cancelAdd();
    } catch (e) {
      addError = String(e);
      showManual = true;
    }
  }

  async function submitManual(e: Event) {
    e.preventDefault();
    if (!dir || !repo) return;
    addError = null;
    try {
      await onOpenProject(dir, repo);
      cancelAdd();
    } catch (e) {
      addError = String(e);
    }
  }

  function dotClass(forge: string): string {
    if (forge === "gitlab") return "dot gl";
    if (forge === "gitea") return "dot ge";
    return "dot gh";
  }
</script>

<div class="sidebar-wrap">
  <aside class="sidebar" style={`width:${width}px`}>
    <div class="section-label">Projects</div>
    <div class="projects">
      {#if project}
        <div class="project on">
          <span class={dotClass(project.forge)}></span>
          <span class="name">{project.name}</span>
        </div>
      {:else}
        <div class="empty">No project loaded</div>
      {/if}

      {#if adding}
        <div class="add-form">
          {#if !showManual}
            <button type="button" class="pick" onclick={pickDir}>Choose folder...</button>
            <div class="add-actions">
              <button type="button" onclick={cancelAdd}>Cancel</button>
            </div>
          {:else}
            <form class="manual-form" onsubmit={submitManual}>
              <div class="picked-dir">{dir}</div>
              {#if addError}
                <div class="add-error">{addError}</div>
              {/if}
              <input class="repo-input" placeholder="owner/repo" bind:value={repo} />
              <div class="add-actions">
                <button type="submit" class="primary">Load</button>
                <button type="button" onclick={cancelAdd}>Cancel</button>
              </div>
            </form>
          {/if}
        </div>
      {:else}
        <button class="add" onclick={beginAdd}>+ Add project</button>
      {/if}
    </div>

    <nav class="nav">
      <div class="nav-item on">Board</div>
      <div class="nav-item soon">Orchestrator (soon)</div>
      <div class="nav-item soon">Subsessions (soon)</div>
    </nav>
  </aside>
  <Resizer onResize={onResize} ariaLabel="Resize sidebar" />
</div>

<style>
  .sidebar-wrap {
    flex: none;
    display: flex;
    border-right: 1px solid var(--border);
    height: 100%;
  }
  .sidebar {
    flex: none;
    background: var(--bg-panel);
    padding: 12px 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    height: 100%;
  }
  .section-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin-bottom: 4px;
  }
  .projects {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .project {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 6px;
    border-radius: 6px;
  }
  .project.on {
    background: var(--active);
    font-weight: 600;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #94a3b8;
    flex: none;
  }
  .dot.gh {
    background: #8957e5;
  }
  .dot.gl {
    background: #e24329;
  }
  .dot.ge {
    background: #f97316;
  }
  .empty {
    font-size: 11px;
    color: var(--text-faint);
    padding: 5px 6px;
  }
  .add {
    text-align: left;
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 11px;
    padding: 5px 6px;
    cursor: pointer;
    border-radius: 6px;
  }
  .add:hover {
    background: var(--hover);
  }
  .add-form {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 6px;
    margin-top: 2px;
  }
  .manual-form {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .pick,
  .repo-input {
    font-size: 11px;
    padding: 5px 6px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    text-align: left;
  }
  .pick {
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .picked-dir {
    font-size: 11px;
    padding: 5px 6px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .add-error {
    font-size: 10px;
    color: #ef4444;
    line-height: 1.4;
  }
  .add-actions {
    display: flex;
    gap: 4px;
  }
  .add-actions button {
    flex: 1;
    font-size: 11px;
    padding: 4px 0;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    cursor: pointer;
  }
  .add-actions .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .nav {
    margin-top: auto;
    font-size: 11px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav-item {
    padding: 4px 6px;
    border-radius: 6px;
  }
  .nav-item.on {
    background: var(--hover);
    font-weight: 600;
  }
  .nav-item.soon {
    color: var(--text-faint);
  }
</style>
