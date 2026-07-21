<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Issue detail panel. It sits beside the board (pushing it, not covering it) and takes
     the roadmap rail's place while an issue is selected. Closing clears the selection. -->
<script lang="ts">
  import type { IssueDetail } from "$lib/ipc";

  let {
    issue,
    loading,
    onClose,
  }: {
    issue: IssueDetail | null;
    loading: boolean;
    onClose: () => void;
  } = $props();

  const statusLabel: Record<string, string> = {
    ready: "ready",
    in_progress: "in progress",
    done: "done",
    other: "untriaged",
  };

  // Color a status-convention label (status:done / status::in-progress, etc.) to match
  // the status badge palette; other labels get a neutral chip.
  function labelKind(name: string): "ready" | "progress" | "done" | "blocked" | "plain" {
    const m = name.match(/^status::?(.+)$/i);
    if (!m) return "plain";
    const v = m[1].toLowerCase();
    if (v.includes("done")) return "done";
    if (v.includes("progress")) return "progress";
    if (v.includes("ready")) return "ready";
    if (v.includes("need") || v.includes("block")) return "blocked";
    return "plain";
  }
</script>

<aside class="drawer">
  <div class="drawer-top">
    <button class="close" onclick={onClose} aria-label="Close">Close</button>
  </div>
  {#if loading || !issue}
    <div class="loading">Loading...</div>
  {:else}
    <div class="content">
      <div class="issue-title">#{issue.id} {issue.title}</div>
      <span class={`badge ${issue.status}`}>{statusLabel[issue.status]}</span>

      <div class="kv"><b>Milestone</b>{issue.milestone ?? "None"}</div>
      <div class="kv labels-row">
        <b>Labels</b>
        {#if issue.labels.length}
          <span class="chips">
            {#each issue.labels as name (name)}
              <span class={`chip ${labelKind(name)}`}>{name}</span>
            {/each}
          </span>
        {:else}
          None
        {/if}
      </div>
      <div class="kv"><b>Branch</b>{issue.branch}</div>

      <div class="col-h">Description</div>
      <div class="body-text">{issue.body || "No description."}</div>
    </div>
  {/if}
</aside>

<style>
  .drawer {
    flex: 0 0 340px;
    min-width: 0;
    height: 100%;
    background: var(--bg-panel);
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    animation: slide-in 0.16s ease-out;
  }
  @keyframes slide-in {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
    to {
      transform: translateX(0);
      opacity: 1;
    }
  }
  .drawer-top {
    display: flex;
    justify-content: flex-end;
    padding: 8px;
    border-bottom: 1px solid var(--border);
  }
  .close {
    background: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 10px;
    font-size: 11px;
    color: var(--text);
    cursor: pointer;
  }
  .loading {
    padding: 16px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .content {
    padding: 14px 16px;
    overflow-y: auto;
    font-size: 12px;
  }
  .issue-title {
    font-size: 14px;
    font-weight: 600;
    margin-bottom: 8px;
  }
  .badge {
    display: inline-block;
    font-size: 10px;
    padding: 2px 9px;
    border-radius: 10px;
    border: 1px solid var(--accent-border);
    background: var(--accent-bg);
    margin-bottom: 10px;
  }
  .badge.done {
    border-color: var(--done);
    background: rgba(34, 197, 94, 0.12);
  }
  .badge.other {
    border-color: var(--border);
    background: var(--hover);
  }
  .kv {
    font-size: 11px;
    margin: 5px 0;
  }
  .kv b {
    color: var(--text-faint);
    font-weight: 600;
    margin-right: 6px;
  }
  .labels-row {
    display: flex;
    align-items: baseline;
    gap: 4px;
    flex-wrap: wrap;
  }
  .chips {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    font-size: 10px;
    padding: 1px 8px;
    border-radius: 10px;
    border: 1px solid var(--border);
    background: var(--hover);
    color: var(--text);
  }
  .chip.ready {
    border-color: var(--accent-border);
    background: var(--accent-bg);
  }
  .chip.progress {
    border-color: #f59e0b;
    background: rgba(245, 158, 11, 0.14);
  }
  .chip.done {
    border-color: var(--done);
    background: rgba(34, 197, 94, 0.14);
  }
  .chip.blocked {
    border-color: #ef4444;
    background: rgba(239, 68, 68, 0.14);
  }
  .col-h {
    font-size: 11px;
    font-weight: 600;
    margin-top: 14px;
    margin-bottom: 6px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .body-text {
    font-size: 11px;
    line-height: 1.6;
    color: var(--text);
    white-space: pre-wrap;
  }
</style>
