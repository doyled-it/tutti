<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Project name, the Board|Lanes segmented toggle, Run/Pause, and the live run status line. -->
<script lang="ts">
  import type { ProjectSummary } from "$lib/ipc";
  import type { RunUi } from "$lib/stores";

  let {
    project,
    view,
    runStatus,
    onViewChange,
    onRun,
    onPause,
  }: {
    project: ProjectSummary | null;
    view: "board" | "lanes";
    runStatus: RunUi;
    onViewChange: (v: "board" | "lanes") => void;
    onRun: () => void;
    onPause: () => void;
  } = $props();

  const statusLabel: Record<RunUi["state"], string> = {
    idle: "Idle",
    running: "Running",
    pausing: "Pausing...",
  };
</script>

<div class="top-bar">
  <strong class="title">{project ? project.repo : "Tutti"}</strong>

  <div class="seg" role="tablist">
    <button
      type="button"
      role="tab"
      aria-selected={view === "board"}
      class:on={view === "board"}
      onclick={() => onViewChange("board")}>Board</button
    >
    <button
      type="button"
      role="tab"
      aria-selected={view === "lanes"}
      class:on={view === "lanes"}
      onclick={() => onViewChange("lanes")}>Lanes</button
    >
  </div>

  <div class="run-bar">
    <span class="status-line">
      <span class="dot" class:live={runStatus.state === "running"}></span>
      {statusLabel[runStatus.state]}
      {#if runStatus.current}
        &middot; {runStatus.current}
      {/if}
      &middot; {runStatus.shipped} shipped
    </span>
    {#if runStatus.state === "idle"}
      <button class="btn primary" disabled={!project} onclick={onRun}>Run</button>
    {:else}
      <button class="btn" disabled={runStatus.state === "pausing"} onclick={onPause}>
        Pause
      </button>
    {/if}
  </div>
</div>

<style>
  .top-bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
  }
  .title {
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    font-size: 12px;
  }
  .seg button {
    padding: 3px 12px;
    cursor: pointer;
    user-select: none;
    background: none;
    border: none;
    color: var(--text);
    font-size: inherit;
    font-family: inherit;
  }
  .seg button.on {
    background: var(--accent);
    color: #fff;
  }
  .run-bar {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .status-line {
    font-size: 11px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-faint);
  }
  .dot.live {
    background: var(--accent);
  }
  .btn {
    background: transparent;
    border: 1px solid var(--border);
    padding: 4px 12px;
    border-radius: 6px;
    font-size: 12px;
    color: var(--text);
    cursor: pointer;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
</style>
