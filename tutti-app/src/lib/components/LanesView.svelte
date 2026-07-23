<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Milestone swimlane: the selected milestone's issues as status-colored chips. Increment 1
     only has children loaded for the selected milestone, so this renders one lane; picking a
     different milestone from the rail swaps which lane's data is shown. -->
<script lang="ts">
  import type { Board, IssueCard } from "$lib/ipc";

  let {
    board,
    onSelectIssue,
  }: {
    board: Board;
    onSelectIssue: (id: number) => void;
  } = $props();

  const milestoneTitle = $derived(
    board.milestones.find((m) => m.id === board.selected_milestone)?.title ?? "Unassigned",
  );

  const chips = $derived([
    ...board.ready.map((c) => ({ card: c, cls: "r" })),
    ...board.in_progress.map((c) => ({ card: c, cls: "i" })),
    ...board.done.map((c) => ({ card: c, cls: "d" })),
  ]);
</script>

<div class="lanes">
  <div class="lane">
    <div class="lane-title">{milestoneTitle}</div>
    <div class="chips">
      {#each chips as { card, cls } (card.id)}
        <button class={`chip ${cls}`} onclick={() => onSelectIssue(card.id)}>
          #{card.id}
          {card.title}
          {#if card.status === "in_progress"}
            <span class="live-dot"></span>
          {/if}
        </button>
      {/each}
      {#if chips.length === 0}
        <div class="empty">No issues in this milestone</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .lanes {
    flex: 1;
    padding: 16px;
    overflow: auto;
  }
  .lane {
    border-bottom: 1px solid var(--border);
    padding-bottom: 12px;
    margin-bottom: 12px;
  }
  .lane-title {
    font-size: 12px;
    font-weight: 600;
    margin-bottom: 8px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
  }
  .chip.r {
    border-color: #64748b;
  }
  .chip.i {
    border-color: var(--accent-border);
    background: var(--accent-bg);
  }
  .chip.d {
    opacity: 0.5;
  }
  .live-dot {
    display: inline-block;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
    margin-left: 4px;
    vertical-align: middle;
  }
  .empty {
    font-size: 11px;
    color: var(--text-faint);
  }
</style>
