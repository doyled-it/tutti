<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- App shell: Sidebar (left) + TopBar/Board-or-Lanes (center) + RoadmapRail (right), with
     the IssueDrawer sliding in from the right over the rail. Live engine events fold into
     the board/runStatus stores via applyEvent. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "$lib/ipc";
  import type { IssueDetail } from "$lib/ipc";
  import {
    project,
    board,
    runStatus,
    applyEvent,
    selectedIssueId,
    view,
  } from "$lib/stores";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import TopBar from "$lib/components/TopBar.svelte";
  import RoadmapRail from "$lib/components/RoadmapRail.svelte";
  import BoardView from "$lib/components/BoardView.svelte";
  import LanesView from "$lib/components/LanesView.svelte";
  import IssueDrawer from "$lib/components/IssueDrawer.svelte";

  let issueDetail = $state<IssueDetail | null>(null);
  let issueLoading = $state(false);
  let loadError = $state<string | null>(null);

  async function openProject(dir: string, repo: string) {
    loadError = null;
    try {
      const summary = await api.loadProject(dir, repo);
      project.set(summary);
      board.set(await api.getBoard());
    } catch (e) {
      loadError = String(e);
    }
  }

  async function selectMilestone(id: number) {
    try {
      board.set(await api.getBoard(id));
    } catch (e) {
      loadError = String(e);
    }
  }

  async function selectIssue(id: number) {
    selectedIssueId.set(id);
    issueLoading = true;
    issueDetail = null;
    try {
      issueDetail = await api.getIssue(id);
    } catch (e) {
      loadError = String(e);
    } finally {
      issueLoading = false;
    }
  }

  function closeDrawer() {
    selectedIssueId.set(null);
    issueDetail = null;
  }

  async function run() {
    try {
      await api.startRun();
      // Optimistic + per-run reset: show running immediately and zero the shipped count
      // for this run (the backend confirms via DrainStarted, and ends via run-ended).
      runStatus.set({ state: "running", shipped: 0 });
    } catch (e) {
      loadError = String(e);
    }
  }

  async function pause() {
    try {
      await api.pauseRun();
    } catch (e) {
      loadError = String(e);
    }
  }

  onMount(() => {
    const progressPromise = api.onProgress(async (ev) => {
      const { board: nb, run: nr } = applyEvent($board, $runStatus, ev);
      board.set(nb);
      runStatus.set(nr);
      if (ev.kind === "drain_complete") {
        try {
          board.set(await api.getBoard($board?.selected_milestone ?? undefined));
        } catch {
          // No project loaded, or the read failed; the live-event board state stands.
        }
      }
    });
    // The run's true end (any exit path, including an engine error): leave the running
    // state and reconcile the board one last time against forge truth.
    const endedPromise = api.onRunEnded(async () => {
      runStatus.update((r) => ({ ...r, state: "idle", current: undefined }));
      try {
        board.set(await api.getBoard($board?.selected_milestone ?? undefined));
      } catch {
        // No project loaded, or the read failed; the current board state stands.
      }
    });
    return () => {
      progressPromise.then((unlisten) => unlisten());
      endedPromise.then((unlisten) => unlisten());
    };
  });
</script>

<div class="shell">
  <Sidebar project={$project} onOpenProject={openProject} />

  <div class="center">
    <TopBar
      project={$project}
      view={$view}
      runStatus={$runStatus}
      onViewChange={(v) => view.set(v)}
      onRun={run}
      onPause={pause}
    />

    {#if loadError}
      <div class="error-banner">{loadError}</div>
    {/if}

    <div class="work">
      {#if $board}
        {#if $view === "board"}
          <BoardView board={$board} onSelectIssue={selectIssue} />
        {:else}
          <LanesView board={$board} onSelectIssue={selectIssue} />
        {/if}
        <RoadmapRail
          milestones={$board.milestones}
          selected={$board.selected_milestone}
          onSelect={selectMilestone}
        />
        <IssueDrawer issue={issueDetail} loading={issueLoading} onClose={closeDrawer} />
      {:else}
        <div class="no-project">Open a project from the sidebar to see its board.</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .shell {
    display: flex;
    height: 100vh;
    width: 100vw;
    overflow: hidden;
  }
  .center {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .work {
    flex: 1;
    display: flex;
    position: relative;
    min-height: 0;
  }
  .no-project {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-faint);
    font-size: 13px;
  }
  .error-banner {
    font-size: 11px;
    padding: 6px 14px;
    background: rgba(239, 68, 68, 0.12);
    color: #ef4444;
    border-bottom: 1px solid var(--border);
  }
</style>
