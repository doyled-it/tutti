<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Kanban board: Ready / In progress / Done columns for the selected milestone. -->
<script lang="ts">
  import type { Board, IssueCard } from "$lib/ipc";

  let {
    board,
    onSelectIssue,
  }: {
    board: Board;
    onSelectIssue: (id: number) => void;
  } = $props();

  const columns = $derived([
    ...(board.untriaged.length > 0 ? [{ key: "untriaged" as const, label: "Untriaged" }] : []),
    { key: "ready" as const, label: "Ready" },
    { key: "in_progress" as const, label: "In progress" },
    { key: "done" as const, label: "Done" },
  ] satisfies {
    key: keyof Pick<Board, "ready" | "in_progress" | "done" | "untriaged">;
    label: string;
  }[]);
</script>

<div class="board">
  {#each columns as col (col.key)}
    <div class="column">
      <div class="col-h">{col.label}</div>
      <div class="cards">
        {#each board[col.key] as card (card.id)}
          <button
            class="card"
            class:un={card.status === "untriaged"}
            class:ip={card.status === "in_progress"}
            class:dn={card.status === "done"}
            onclick={() => onSelectIssue(card.id)}
          >
            <span class="card-id">#{card.id}</span>
            {card.title}
            {#if card.status === "in_progress"}
              <span class="live-dot"></span>
            {/if}
          </button>
        {/each}
        {#if board[col.key].length === 0}
          <div class="empty">Nothing here</div>
        {/if}
      </div>
    </div>
  {/each}
</div>

<style>
  .board {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: flex-start;
    gap: 16px;
    padding: 16px;
    /* Scroll left/right when the columns do not fit, and down for long lists. */
    overflow: auto;
  }
  .column {
    /* Fixed-width columns so the board scrolls horizontally instead of squishing. */
    flex: 0 0 280px;
    display: flex;
    flex-direction: column;
  }
  .col-h {
    font-size: 11px;
    font-weight: 600;
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .card {
    text-align: left;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 12px;
    background: var(--bg-panel);
    color: var(--text);
    cursor: pointer;
    position: relative;
  }
  .card:hover {
    border-color: var(--accent-border);
  }
  .card.ip {
    border-color: var(--accent-border);
    background: var(--accent-bg);
  }
  .card.un {
    border-style: dashed;
    border-color: var(--border);
    color: var(--text-dim);
  }
  .card.dn {
    opacity: 0.55;
  }
  .card-id {
    color: var(--text-faint);
    margin-right: 4px;
  }
  .live-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    margin-left: 6px;
    vertical-align: middle;
  }
  .empty {
    font-size: 11px;
    color: var(--text-faint);
    padding: 6px 2px;
  }
</style>
