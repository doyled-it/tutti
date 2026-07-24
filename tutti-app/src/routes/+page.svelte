<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- App shell: Sidebar (left) + TopBar/Board-or-Lanes (center) + RoadmapRail (right), with
     the IssueDrawer sliding in from the right over the rail. Live engine events fold into
     the board/runStatus stores via applyEvent. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { get } from "svelte/store";
  import { api } from "$lib/ipc";
  import type { InitForm, IssueDetail, Probe } from "$lib/ipc";
  import {
    projects,
    activeDir,
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
  import InitWizard from "$lib/components/InitWizard.svelte";
  import BrowseForge from "$lib/components/BrowseForge.svelte";
  import CreateRepo from "$lib/components/CreateRepo.svelte";

  let issueDetail = $state<IssueDetail | null>(null);
  let issueLoading = $state(false);
  let loadError = $state<string | null>(null);

  // Set when the user picks a folder with no tutti.toml; opens the wizard over the shell.
  // `login` pre-fills the wizard's Gitea login when we already know it (browse flow).
  let pendingInit = $state<{ dir: string; probe: Probe; login: string } | null>(null);

  function onNeedsInit(dir: string, probe: Probe, login = "") {
    pendingInit = { dir, probe, login };
  }

  // Set while the browse-a-forge modal is open.
  let browsing = $state(false);

  function onBrowse() {
    browsing = true;
  }

  // Set while the create-a-new-repo modal is open.
  let creating = $state(false);

  function onCreate() {
    creating = true;
  }

  // After a browse clone lands a local checkout, run it through the same probe-then-add
  // or probe-then-wizard path a manually picked folder uses. The browse flow already knows
  // the forge kind and (for Gitea) the login, so carry both through rather than making the
  // wizard re-derive the forge from the remote host (which only recognizes the three
  // public hosts, so a self-hosted GitLab/Gitea/GHE would come back unknown).
  async function onCloned(dir: string, forgeKind: string, login: string) {
    browsing = false;
    creating = false;
    loadError = null;
    try {
      const probe = await api.probeProject(dir);
      if (probe.has_config) {
        await onAdd(dir, probe.repo ?? undefined);
      } else {
        const known = { ...probe, forge_kind: probe.forge_kind ?? forgeKind };
        onNeedsInit(dir, known, forgeKind === "gitea" ? login : "");
      }
    } catch (e) {
      loadError = String(e);
    }
  }

  // Re-open the folder picker from inside the wizard, replacing the pending target. The
  // wizard stays mounted and re-seeds itself from the new dir, so do not null it out here.
  async function repickInit() {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({ directory: true });
    if (typeof picked !== "string") return;
    try {
      const probe = await api.probeProject(picked);
      if (probe.has_config) {
        // It is already a Tutti project: just add it and close the wizard.
        pendingInit = null;
        await onAdd(picked, probe.repo ?? undefined);
      } else {
        pendingInit = { dir: picked, probe, login: "" };
      }
    } catch (e) {
      loadError = String(e);
    }
  }

  // Switch the active project: activate it on the backend (this also persists the choice),
  // load its board, and close any open issue drawer since it belongs to the old project.
  async function switchTo(dir: string) {
    try {
      await api.switchProject(dir);
    } catch (e) {
      loadError = String(e);
      throw e;
    }
    activeDir.set(dir);
    board.set(await api.getBoard());
    selectedIssueId.set(null);
  }

  async function onSwitch(dir: string) {
    loadError = null;
    try {
      await switchTo(dir);
    } catch {
      // switchTo already recorded loadError.
    }
  }

  // Called by the sidebar's "add project" flow. On the common path (folder has a
  // detectable git origin) this is a single call with no repo. When there is no remote,
  // or detection fails, the sidebar retries with a manually entered repo. Rethrows so the
  // sidebar can decide whether to reveal the manual-entry fallback; also mirrors the
  // message into the page-level banner so a hard failure (e.g. a bad manual repo) stays
  // visible.
  async function onAdd(dir: string, repo?: string) {
    loadError = null;
    try {
      const entry = await api.addProject(dir, repo);
      const list = await api.listProjects();
      projects.set(list.projects);
      activeDir.set(entry.dir);
      // The board is now a different project; close any open issue drawer so it does not
      // show the previous project's issue over the new board.
      selectedIssueId.set(null);
      issueDetail = null;
      board.set(await api.getBoard());
    } catch (e) {
      loadError = String(e);
      throw e;
    }
  }

  // Called by the wizard's final Create step when the picked folder has no tutti.toml.
  // Writes the config on the backend, then activates and loads the new project the same
  // way onAdd does for an existing one, and closes the wizard. Rethrows on failure so the
  // wizard keeps the answers on screen and shows the message.
  async function onInit(form: InitForm) {
    loadError = null;
    try {
      const entry = await api.initProject(form);
      const list = await api.listProjects();
      projects.set(list.projects);
      activeDir.set(entry.dir);
      selectedIssueId.set(null);
      issueDetail = null;
      board.set(await api.getBoard());
      pendingInit = null;
    } catch (e) {
      // Deliberately not mirrored into loadError: the wizard is still open over the
      // shell and shows the message itself, and a copy in the page banner would sit
      // behind the scrim and then linger unexplained after Cancel.
      throw e;
    }
  }

  async function onRemove(dir: string) {
    loadError = null;
    try {
      await api.removeProject(dir);
      const list = await api.listProjects();
      projects.set(list.projects);
      if (dir === get(activeDir)) {
        board.set(null);
        activeDir.set(null);
        selectedIssueId.set(null);
        issueDetail = null;
      }
    } catch (e) {
      loadError = String(e);
    }
  }

  async function selectMilestone(id: number | null) {
    try {
      board.set(await api.getBoard(id ?? undefined));
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
      // Close the drawer on error so it does not sit on a false "Loading..." state; the
      // error surfaces in the top banner.
      selectedIssueId.set(null);
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

  // The active project's full entry, looked up from the list; used by the header title.
  let activeEntry = $derived($projects.find((p) => p.dir === $activeDir) ?? null);

  onMount(() => {
    // Restore the saved project list and, if there was an active one, its board. A stale
    // active entry (e.g. a moved folder) must not block launch: swallow the error so the
    // rest of the list still renders and the user can remove the bad entry.
    (async () => {
      try {
        const list = await api.listProjects();
        projects.set(list.projects);
        activeDir.set(list.active);
        if (list.active) {
          await switchTo(list.active);
        }
      } catch (e) {
        loadError = String(e);
      }
    })();

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
  <Sidebar
    projects={$projects}
    activeDir={$activeDir}
    runActive={$runStatus.state !== "idle"}
    {onSwitch}
    {onAdd}
    {onNeedsInit}
    {onBrowse}
    {onCreate}
    {onRemove}
  />

  <div class="center">
    <TopBar
      project={activeEntry}
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
        {#if $selectedIssueId !== null}
          <IssueDrawer issue={issueDetail} loading={issueLoading} onClose={closeDrawer} />
        {:else}
          <RoadmapRail
            milestones={$board.milestones}
            selected={$board.selected_milestone}
            onSelect={selectMilestone}
          />
        {/if}
      {:else}
        <div class="no-project">Open a project from the sidebar to see its board.</div>
      {/if}
    </div>
  </div>
</div>

{#if pendingInit}
  <InitWizard
    dir={pendingInit.dir}
    probe={pendingInit.probe}
    login={pendingInit.login}
    onCancel={() => (pendingInit = null)}
    onCreate={onInit}
    onRepick={repickInit}
  />
{/if}

{#if browsing}
  <BrowseForge onCancel={() => (browsing = false)} {onCloned} />
{/if}

{#if creating}
  <CreateRepo onCancel={() => (creating = false)} {onCloned} />
{/if}

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
