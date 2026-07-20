<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Slide-in detail drawer from the right, over the roadmap rail. The board stays live behind
     it; closing just clears the selection. -->
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
</script>

{#if issue !== null || loading}
  <div
    class="scrim"
    role="button"
    tabindex="0"
    aria-label="Close issue detail"
    onclick={onClose}
    onkeydown={(e) => (e.key === "Enter" || e.key === "Escape") && onClose()}
  ></div>
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
        <div class="kv"><b>Labels</b>{issue.labels.length ? issue.labels.join(", ") : "None"}</div>
        <div class="kv"><b>Branch</b>{issue.branch}</div>

        <div class="col-h">Description</div>
        <div class="body-text">{issue.body || "No description."}</div>
      </div>
    {/if}
  </aside>
{/if}

<style>
  .scrim {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.15);
    z-index: 5;
  }
  .drawer {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 320px;
    background: var(--bg-panel);
    border-left: 1px solid var(--border);
    box-shadow: -8px 0 24px rgba(0, 0, 0, 0.15);
    z-index: 6;
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
