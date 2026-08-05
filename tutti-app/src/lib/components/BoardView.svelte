<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Kanban board: Untriaged / Needs human / Ready / In progress / Done columns for the
     selected milestone, plus the triage bar that converts a pre-Tutti backlog. Only
     untriaged cards are selectable, which is the UI half of the backend's eligibility
     guardrail. Pure column/selection/summary logic lives in $lib/board.ts. -->
<script lang="ts">
  import { api, type Board, type TriageTarget } from "$lib/ipc";
  import {
    boardColumns,
    triageGap,
    toggleSelected,
    isSelectable,
    toggleColumn,
    columnFullySelected,
    pruneSelection,
    triageSummary,
  } from "$lib/board";

  let {
    board,
    onSelectIssue,
    onTriaged = null,
  }: {
    board: Board;
    onSelectIssue: (id: number) => void;
    /** Fired after a triage apply so the host can refresh the board. */
    onTriaged?: (() => void) | null;
  } = $props();

  const columns = $derived(boardColumns(board));
  const gap = $derived(triageGap(board));

  let selected = $state<Set<number>>(new Set());
  let applying = $state(false);
  let note = $state<string | null>(null);
  let error = $state<string | null>(null);

  // A refreshed board moves just-labelled issues out of the selectable columns. `pruneSelection`
  // returns the same instance when nothing changed, so this effect writes only when it has real
  // work to do and cannot loop on its own assignment.
  $effect(() => {
    const pruned = pruneSelection(selected, board);
    if (pruned !== selected) selected = pruned;
  });

  async function apply(to: TriageTarget) {
    if (selected.size === 0 || applying) return;
    applying = true;
    error = null;
    note = null;
    try {
      const outcome = await api.applyTriage([...selected], to);
      note = triageSummary(outcome, to);
      selected = new Set();
      onTriaged?.();
    } catch (e) {
      error = String(e);
    } finally {
      applying = false;
    }
  }
</script>

<!-- A column wrapper: the host `.work` is a flex ROW (board beside the drawer/rail), so the
     gap banner and triage bar must stack with the columns, not sit next to them. -->
<div class="board-pane">
  {#if gap.untriaged > 0}
    <!-- The headline a freshly added pre-Tutti repo needs: the board and the engine disagree
         about how much work is actually available, and this says by how much. -->
    <div class="gap">
      <span class="gap-figure">{gap.total} issues</span>
      <span class="gap-sep">·</span>
      <span class="gap-figure ok">{gap.ready} ready</span>
      <span class="gap-sep">·</span>
      <span class="gap-figure warn">{gap.untriaged} untriaged</span>
      <span class="gap-note">A run picks up only the ready ones.</span>
    </div>
  {/if}

  <div class="board">
    {#each columns as col (col.key)}
      <div class="column">
        <div class="col-h">
          <span>{col.label}</span>
          <span class="col-count">{board[col.key].length}</span>
          {#if isSelectable(col.key)}
            <button
              class="col-action"
              onclick={() => (selected = toggleColumn(selected, board, col.key))}
            >
              {columnFullySelected(selected, board, col.key) ? "Clear" : "Select all"}
            </button>
          {/if}
        </div>
        <div class="cards">
          {#each board[col.key] as card (card.id)}
            {#if isSelectable(col.key)}
              <!-- A checkbox cannot be nested inside the title button, so a triageable card is
                   a row: a real checkbox plus the button that opens the drawer. -->
              <div class="row" class:picked={selected.has(card.id)}>
                <input
                  type="checkbox"
                  checked={selected.has(card.id)}
                  aria-label={`Select issue ${card.id}`}
                  onchange={() => (selected = toggleSelected(selected, card.id))}
                />
                <button
                  class="card row-card"
                  class:un={card.status === "untriaged"}
                  class:hu={card.status === "needs_human"}
                  onclick={() => onSelectIssue(card.id)}
                >
                  <span class="card-id">#{card.id}</span>
                  {card.title}
                </button>
              </div>
            {:else}
              <button
                class="card"
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
            {/if}
          {/each}
          {#if board[col.key].length === 0}
            <div class="empty">Nothing here</div>
          {/if}
        </div>
      </div>
    {/each}
  </div>

  {#if selected.size > 0 || note || error}
    <div class="triage-bar">
      {#if selected.size > 0}
        <span class="sel">{selected.size} selected</span>
        <button class="act" disabled={applying} onclick={() => apply("ready")}>Mark ready</button>
        <button class="act" disabled={applying} onclick={() => apply("needs_human")}>
          Park (needs human)
        </button>
        <button class="act ghost" disabled={applying} onclick={() => (selected = new Set())}>
          Cancel
        </button>
      {/if}
      {#if note}<span class="note">{note}</span>{/if}
      {#if error}<span class="err">{error}</span>{/if}
    </div>
  {/if}
</div>

<style>
  .board-pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .gap {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .gap-figure {
    font-weight: 600;
  }
  .gap-figure.ok {
    color: var(--accent);
  }
  .gap-figure.warn {
    color: #e3b341;
  }
  .gap-sep {
    color: var(--text-faint);
  }
  .gap-note {
    color: var(--text-dim);
    margin-left: 4px;
  }
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
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-weight: 600;
    margin-bottom: 8px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .col-count {
    color: var(--text-faint);
    font-weight: 500;
  }
  .col-action {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--accent);
    font: inherit;
    font-size: 10px;
    text-transform: none;
    letter-spacing: 0;
    cursor: pointer;
    padding: 0;
  }
  .cards {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .row input {
    flex: none;
    cursor: pointer;
    accent-color: var(--accent);
  }
  .row-card {
    flex: 1;
    min-width: 0;
  }
  .row.picked .row-card {
    border-style: solid;
    border-color: var(--accent-border);
    background: var(--accent-bg);
    color: var(--text);
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
  .card.hu {
    border-color: #e3b341;
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
  .triage-bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 8px 16px;
    border-top: 1px solid var(--border);
    background: var(--bg-panel);
    font-size: 12px;
  }
  .sel {
    font-weight: 600;
  }
  .act {
    border: 1px solid var(--accent-border);
    background: var(--accent-bg);
    color: var(--text);
    border-radius: 6px;
    padding: 4px 10px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .act:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .act.ghost {
    border-color: var(--border);
    background: none;
    color: var(--text-dim);
  }
  .note {
    color: var(--text-dim);
  }
  .err {
    color: #ff8c6b;
  }
</style>
