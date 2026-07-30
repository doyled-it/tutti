<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Subsessions pane: a live, read-only master-detail view of the current run. Left is a
     list of per-role subsessions (one per issue+role) with a status dot; right is the selected
     subsession's streamed transcript rendered like the orchestrator chat, plus its outcome
     footer. No compose box: it observes the run, it never drives it. State lives in the
     `subsessions` store; the pure reducer is in $lib/subsessions.ts. -->
<script lang="ts">
  import { subsessions } from "$lib/stores";
  import { roleLabel, selectSubsession, type SubStatus } from "$lib/subsessions";

  let selected = $derived($subsessions.list.find((s) => s.key === $subsessions.selected) ?? null);

  function statusClass(status: SubStatus): string {
    if (status === "done") return "dot done";
    if (status === "error") return "dot error";
    return "dot running";
  }

  // Text equivalent for the status dot, so running/done/error is not color-only information.
  function statusWord(status: SubStatus): string {
    if (status === "done") return "done";
    if (status === "error") return "error";
    return "running";
  }
</script>

<div class="pane">
  {#if $subsessions.list.length === 0}
    <div class="empty">Start a run to watch its stages.</div>
  {:else}
    <div class="list">
      {#each $subsessions.list as s (s.key)}
        <button
          type="button"
          class="row"
          class:on={s.key === $subsessions.selected}
          onclick={() => subsessions.update((st) => selectSubsession(st, s.key))}
        >
          <span class={statusClass(s.status)} aria-label={statusWord(s.status)}></span>
          <span class="row-label">{roleLabel(s)}</span>
        </button>
      {/each}
    </div>

    <div class="detail">
      {#if selected}
        <div class="detail-head">
          <span class="head-label">{roleLabel(selected)}</span>
          <span class="head-title">{selected.title}</span>
        </div>
        <div class="transcript">
          {#each selected.messages as m}
            {#if m.kind === "tool"}
              <div class="msg assistant tool"><span class="tool">ran {m.text}</span></div>
            {:else if m.text !== ""}
              <div class="msg assistant"><div class="bubble">{m.text}</div></div>
            {/if}
          {/each}
        </div>
        {#if selected.summary}
          <div class="footer {selected.status}">
            <span class="glyph"
              >{selected.status === "done" ? "✓" : selected.status === "error" ? "⚠" : "…"}</span
            >
            {selected.summary}
          </div>
        {/if}
      {:else}
        <div class="empty">Select a subsession.</div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .pane {
    --status-running: var(--accent);
    --status-done: #5fd3c4;
    --status-error: #ff8c6b;
    flex: 1;
    display: flex;
    min-width: 0;
    min-height: 0;
  }
  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    font-size: 13px;
  }
  .list {
    flex: none;
    width: 200px;
    border-right: 1px solid var(--border);
    padding: 10px 8px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
    border-radius: 6px;
    width: 100%;
    background: none;
    border: none;
    color: var(--text);
    font: inherit;
    font-size: 12px;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.on {
    background: var(--active);
    font-weight: 600;
  }
  .row-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
    background: var(--text-faint);
  }
  .dot.running {
    background: var(--status-running);
  }
  .dot.done {
    background: var(--status-done);
  }
  .dot.error {
    background: var(--status-error);
  }
  .detail {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .detail-head {
    flex: none;
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
  }
  .head-label {
    font-weight: 600;
    font-size: 13px;
  }
  .head-title {
    color: var(--text-dim);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .transcript {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .msg {
    display: flex;
  }
  .bubble {
    max-width: 70%;
    padding: 8px 12px;
    border-radius: 10px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    white-space: pre-wrap;
    font-size: 13px;
    line-height: 1.5;
  }
  .tool {
    font-size: 11px;
    color: var(--text-faint);
    font-family: monospace;
  }
  .footer {
    flex: none;
    padding: 8px 16px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .footer.done .glyph {
    color: var(--status-done);
  }
  .footer.error .glyph {
    color: var(--status-error);
  }
</style>
