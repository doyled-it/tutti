<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Pinned-right roadmap: one row per milestone with a progress bar. Clicking a row
     re-fetches the board for that milestone. -->
<script lang="ts">
  import type { MilestoneRow } from "$lib/ipc";

  let {
    milestones,
    selected,
    onSelect,
  }: {
    milestones: MilestoneRow[];
    selected: number | null;
    onSelect: (id: number) => void;
  } = $props();

  function pct(m: MilestoneRow): number {
    if (m.total === 0) return 0;
    return Math.round((m.done / m.total) * 100);
  }
</script>

<aside class="rail">
  <div class="col-h">Roadmap</div>
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
</aside>

<style>
  .rail {
    width: 160px;
    flex: none;
    border-left: 1px solid var(--border);
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
