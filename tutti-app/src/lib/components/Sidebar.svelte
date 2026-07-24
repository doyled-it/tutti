<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Left rail: the persisted project list, an "add project" affordance, per-row switch and
     remove, and the primary nav (Board is live; Orchestrator/Subsessions are placeholders).
     Switching is disabled while a run is active. Resizable via a drag handle on the right
     edge, with the width persisted to localStorage. -->
<script lang="ts">
  import { api } from "$lib/ipc";
  import type { Probe, ProjectEntry } from "$lib/ipc";
  import Resizer from "./Resizer.svelte";

  let {
    projects,
    activeDir,
    runActive,
    onSwitch,
    onAdd,
    onNeedsInit,
    onBrowse,
    onCreate,
    onRemove,
  }: {
    projects: ProjectEntry[];
    activeDir: string | null;
    runActive: boolean;
    onSwitch: (dir: string) => void;
    onAdd: (dir: string, repo?: string) => Promise<void>;
    onNeedsInit: (dir: string, probe: Probe) => void;
    onBrowse: () => void;
    onCreate: () => void;
    onRemove: (dir: string) => void;
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
  let addError = $state<string | null>(null);

  function beginAdd() {
    if (runActive) return;
    adding = true;
    addError = null;
  }

  function cancelAdd() {
    adding = false;
    dir = "";
    addError = null;
  }

  // Pick a folder, then probe it. A folder with a tutti.toml loads immediately via the
  // existing add path. One without hands off to the page, which opens the guided wizard
  // over the whole window, pre-filled from whatever the probe could detect.
  async function pickDir() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true });
    if (typeof picked !== "string") return;
    dir = picked;
    addError = null;
    try {
      const probe = await api.probeProject(dir);
      if (probe.has_config) {
        await onAdd(dir, probe.repo ?? undefined);
        cancelAdd();
      } else {
        // No config here: the page opens the guided wizard over the whole window.
        cancelAdd();
        onNeedsInit(picked, probe);
      }
    } catch (e) {
      addError = String(e);
    }
  }

  function dotClass(forge: string): string {
    if (forge === "gitlab") return "dot gl";
    if (forge === "gitea") return "dot ge";
    return "dot gh";
  }

  // Hand off to the page-owned BrowseForge modal, closing the add affordance behind it.
  function beginBrowse() {
    if (runActive) return;
    cancelAdd();
    onBrowse();
  }

  // Hand off to the page-owned CreateRepo modal, closing the add affordance behind it.
  function beginCreate() {
    if (runActive) return;
    cancelAdd();
    onCreate();
  }

  function handleSwitch(dir: string) {
    if (runActive) return;
    onSwitch(dir);
  }

  function handleRemove(e: MouseEvent, dir: string) {
    e.stopPropagation();
    onRemove(dir);
  }
</script>

<div class="sidebar-wrap">
  <aside class="sidebar" style={`width:${width}px`}>
    <div class="section-label">Projects</div>
    <div class="projects">
      {#if projects.length === 0}
        <div class="empty">No projects yet</div>
      {:else}
        {#each projects as p (p.dir)}
          <button
            type="button"
            class="project"
            class:on={p.dir === activeDir}
            class:disabled={runActive}
            disabled={runActive}
            title={runActive ? "pause the run to switch" : undefined}
            onclick={() => handleSwitch(p.dir)}
          >
            <span class={dotClass(p.forge)}></span>
            <span class="name">{p.name}</span>
            <span
              class="remove"
              role="button"
              tabindex="0"
              title="Remove project"
              onclick={(e) => handleRemove(e, p.dir)}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  handleRemove(e as unknown as MouseEvent, p.dir);
                }
              }}
            >
              &times;
            </span>
          </button>
        {/each}
      {/if}

      {#if adding}
        <div class="add-form">
          <button type="button" class="pick" onclick={pickDir}>Open a local folder...</button>
          <button type="button" class="pick" onclick={beginBrowse}>Browse a forge...</button>
          <button type="button" class="pick" onclick={beginCreate}>Create a new repo...</button>
          {#if addError}
            <div class="add-error">{addError}</div>
          {/if}
          <div class="add-actions">
            <button type="button" onclick={cancelAdd}>Cancel</button>
          </div>
        </div>
      {:else}
        <button
          class="add"
          onclick={beginAdd}
          disabled={runActive}
          title={runActive ? "pause the run to add a project" : undefined}>+ Add project</button
        >
      {/if}
    </div>

    <nav class="nav">
      <div class="nav-item on">Board</div>
      <div class="nav-item soon">Orchestrator (soon)</div>
      <div class="nav-item soon">Subsessions (soon)</div>
    </nav>
  </aside>
  <Resizer {onResize} ariaLabel="Resize sidebar" />
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
    width: 100%;
    background: none;
    border: none;
    color: var(--text);
    font-size: inherit;
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }
  .project:hover {
    background: var(--hover);
  }
  .project.on {
    background: var(--active);
    font-weight: 600;
  }
  .project.disabled {
    cursor: default;
    opacity: 0.7;
  }
  .project.disabled:hover {
    background: none;
  }
  .project.disabled.on:hover {
    background: var(--active);
  }
  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .remove {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    border-radius: 4px;
    font-size: 12px;
    line-height: 1;
    color: var(--text-faint);
    opacity: 0;
    cursor: pointer;
  }
  .project:hover .remove {
    opacity: 1;
  }
  .remove:hover {
    background: var(--hover);
    color: var(--text);
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
  .pick {
    font-size: 11px;
    padding: 5px 6px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--text);
    text-align: left;
    cursor: pointer;
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
