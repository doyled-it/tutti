<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Pinned-right roadmap: an "All issues" entry followed by one row per milestone with a
     progress bar. Clicking "All issues" clears the milestone filter; clicking a row
     re-fetches the board for that milestone. Resizable via a drag handle on the left
     edge, with the width persisted to localStorage. -->
<script lang="ts">
  import type { MilestoneRow } from "$lib/ipc";
  import Resizer from "./Resizer.svelte";

  let {
    milestones,
    selected,
    onSelect,
  }: {
    milestones: MilestoneRow[];
    selected: number | null;
    onSelect: (id: number | null) => void;
  } = $props();

  const WIDTH_KEY = "tutti.railWidth";
  const MIN_WIDTH = 120;
  const MAX_WIDTH = 320;
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
    // The rail sits on the right edge of the window and grows leftward as the handle,
    // on its left edge, is dragged left, so a negative pointer delta increases width.
    width = clamp(width - deltaX);
    localStorage.setItem(WIDTH_KEY, String(width));
  }

  function pct(m: MilestoneRow): number {
    if (m.total === 0) return 0;
    return Math.round((m.done / m.total) * 100);
  }
</script>

<aside class="rail-wrap">
  <Resizer {onResize} ariaLabel="Resize roadmap rail" />
  <div class="rail" style={`width:${width}px`}>
    <div class="col-h">Roadmap</div>
    <div
      class="ms all"
      class:sel={selected == null}
      role="button"
      tabindex="0"
      onclick={() => onSelect(null)}
      onkeydown={(e) => e.key === "Enter" && onSelect(null)}
    >
      All issues
    </div>
    {#if milestones.length === 0}
      <div class="empty">No milestones</div>
    {/if}
    {#each milestones as m (m.id)}
      <div
        class="ms"
        class:sel={m.id === selected}
        class:done={!m.open}
        role="button"
        tabindex="0"
        onclick={() => onSelect(m.id)}
        onkeydown={(e) => e.key === "Enter" && onSelect(m.id)}
      >
        <div class="ms-title">{m.title}</div>
        <div class="prog"><i style={`width:${pct(m)}%`}></i></div>
        <div class="ms-count">{m.done}/{m.total}</div>
      </div>
    {/each}
  </div>
</aside>

<style>
  .rail-wrap {
    flex: none;
    display: flex;
    border-left: 1px solid var(--border);
  }
  .rail {
    flex: none;
    background: var(--bg-panel);
    padding: 12px 10px;
    overflow-y: auto;
  }
  .col-h {
    font-size: 11px;
    font-weight: 600;
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .empty {
    font-size: 11px;
    color: var(--text-faint);
  }
  .ms {
    margin-bottom: 12px;
    font-size: 11px;
    cursor: pointer;
    padding: 4px 6px;
    border-radius: 6px;
  }
  .ms:hover {
    background: var(--hover);
  }
  .ms.sel {
    font-weight: 600;
    background: var(--active);
  }
  .ms.done {
    opacity: 0.6;
  }
  .ms.all {
    padding-bottom: 8px;
    margin-bottom: 14px;
    border-bottom: 1px solid var(--border);
  }
  .ms-title {
    margin-bottom: 3px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .prog {
    height: 5px;
    border-radius: 3px;
    background: rgba(128, 128, 128, 0.25);
    overflow: hidden;
  }
  .prog > i {
    display: block;
    height: 100%;
    background: var(--done);
  }
  .ms-count {
    margin-top: 3px;
    color: var(--text-faint);
    font-size: 10px;
  }
</style>
