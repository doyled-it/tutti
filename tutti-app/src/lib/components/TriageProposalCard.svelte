<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The orchestrator's triage proposal, resolved into something a human can approve.

     The agent proposes bare issue numbers, and it forms them by reading the backlog itself,
     including issue bodies written by anyone who can file an issue on the repo. A card
     showing five integers asks the user to approve text they have never seen, one step
     upstream of an agent that runs with permissions skipped. So this card resolves every
     proposed id against the forge and shows the real title and current status, and it
     refuses to enable Apply until that resolve succeeds: an unverified proposal is exactly
     the one not to approve. -->
<script lang="ts">
  import { api, type TriagePreview, type TriageTarget, type Status } from "$lib/ipc";
  import type { TriageProposal } from "$lib/orchestrator";
  import { triageSummary } from "$lib/board";

  let {
    proposal,
    onApplied = null,
    onDismiss,
  }: {
    proposal: TriageProposal;
    onApplied?: (() => void) | null;
    onDismiss: () => void;
  } = $props();

  type Section = { to: TriageTarget; label: string; ids: number[] };

  const sections = $derived(
    (
      [
        { to: "ready", label: "Mark ready", ids: proposal.ready },
        { to: "needs_human", label: "Park", ids: proposal.needs_human },
      ] as Section[]
    ).filter((s) => s.ids.length > 0),
  );

  let previews = $state<Record<string, TriagePreview[]>>({});
  let resolveError = $state<string | null>(null);
  let applying = $state(false);
  let note = $state<string | null>(null);

  // Resolve on mount, and again after an apply, so the card always describes current state
  // rather than whatever was true when the agent wrote the proposal.
  $effect(() => {
    void proposal;
    resolve();
  });

  async function resolve() {
    resolveError = null;
    try {
      const next: Record<string, TriagePreview[]> = {};
      for (const s of sections) {
        next[s.to] = await api.previewTriage(s.ids, s.to);
      }
      previews = next;
    } catch (e) {
      previews = {};
      resolveError = String(e);
    }
  }

  const eligibleIds = (to: TriageTarget): number[] =>
    (previews[to] ?? []).filter((p) => p.eligible).map((p) => p.id);

  const statusWord = (s: Status | null): string => (s === null ? "not found" : s.replace("_", " "));

  /** A proposed Ready for an issue a human deliberately parked. Allowed, but never silent. */
  const isUnpark = (to: TriageTarget, p: TriagePreview) =>
    to === "ready" && p.status === "needs_human";

  async function apply(to: TriageTarget) {
    const ids = eligibleIds(to);
    if (ids.length === 0 || applying) return;
    applying = true;
    note = null;
    try {
      note = triageSummary(await api.applyTriage(ids, to), to);
      onApplied?.();
      await resolve();
    } catch (e) {
      note = String(e);
    } finally {
      applying = false;
    }
  }
</script>

<div class="proposal">
  <div class="proposal-title">Triage the backlog?</div>

  {#if resolveError}
    <div class="resolve-err">
      Could not read these issues from the forge, so this proposal cannot be checked: {resolveError}
    </div>
  {/if}

  {#each sections as s (s.to)}
    <div class="section">
      <div class="section-head">{s.label}</div>
      {#if previews[s.to]}
        <ul class="rows">
          {#each previews[s.to] as p (p.id)}
            <li class="row" class:skip={!p.eligible}>
              <span class="num">#{p.id}</span>
              <span class="title">{p.title ?? "(no such issue)"}</span>
              <span class="now">{statusWord(p.status)}</span>
              {#if isUnpark(s.to, p)}
                <span class="warn">un-parks</span>
              {:else if !p.eligible}
                <span class="warn muted">no change</span>
              {/if}
            </li>
          {/each}
        </ul>
      {:else if !resolveError}
        <div class="loading">Reading {s.ids.length} issues...</div>
      {/if}
    </div>
  {/each}

  {#if proposal.rationale}
    <div class="proposal-why">{proposal.rationale}</div>
  {/if}

  <!-- The two lists apply separately: a user who agrees the ready set is right but not the
       park set can take one and leave the other. -->
  <div class="proposal-actions">
    {#each sections as s (s.to)}
      <button
        class="apply"
        disabled={applying || !previews[s.to] || eligibleIds(s.to).length === 0}
        onclick={() => apply(s.to)}
      >
        {s.label}
        {eligibleIds(s.to).length || ""}
      </button>
    {/each}
    <button class="dismiss" onclick={onDismiss}>Dismiss</button>
  </div>

  {#if note}<div class="proposal-why">{note}</div>{/if}
</div>

<style>
  .proposal {
    border: 1px solid var(--accent-border);
    background: var(--accent-bg);
    border-radius: 10px;
    padding: 10px 12px;
    max-width: 90%;
  }
  .proposal-title {
    font-size: 13px;
    font-weight: 600;
    margin-bottom: 8px;
  }
  .section {
    margin-bottom: 8px;
  }
  .section-head {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    margin-bottom: 4px;
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: baseline;
    gap: 6px;
    font-size: 12px;
  }
  .row.skip {
    opacity: 0.55;
  }
  .num {
    flex: none;
    font-family: monospace;
    color: var(--text-faint);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .now {
    flex: none;
    font-size: 11px;
    color: var(--text-dim);
  }
  .warn {
    flex: none;
    font-size: 11px;
    color: #e3b341;
  }
  .warn.muted {
    color: var(--text-faint);
  }
  .loading,
  .proposal-why {
    font-size: 11px;
    color: var(--text-dim);
    margin-top: 4px;
  }
  .resolve-err {
    font-size: 11px;
    color: #ff8c6b;
    margin-bottom: 6px;
  }
  .proposal-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
  .apply {
    border: 1px solid var(--accent-border);
    background: var(--bg-panel);
    color: var(--text);
    border-radius: 6px;
    padding: 4px 10px;
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .apply:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .dismiss {
    border: none;
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
</style>
