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

  // Render a label's real forge color as a pill. Scoped labels (GitLab `scope::value`,
  // or the GitHub/Gitea `scope:value` convention) render as a two-tone pill: a solid
  // "scope" half and a tinted "value" half, mirroring GitLab's own scoped-label style.
  function hexToRgb(hex: string): [number, number, number] {
    const h = hex.replace(/^#/, "");
    const full =
      h.length === 3
        ? h
            .split("")
            .map((c) => c + c)
            .join("")
        : h.padEnd(6, "0").slice(0, 6);
    return [parseInt(full.slice(0, 2), 16), parseInt(full.slice(2, 4), 16), parseInt(full.slice(4, 6), 16)];
  }
  function textOn(rgb: [number, number, number]): string {
    const [r, g, b] = rgb;
    const l = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
    return l > 0.6 ? "#1a1a1a" : "#ffffff";
  }
  function splitScoped(name: string): { scope: string; value: string } | null {
    const dd = name.indexOf("::");
    if (dd > 0 && dd + 2 < name.length) return { scope: name.slice(0, dd), value: name.slice(dd + 2) };
    const s = name.indexOf(":");
    if (s > 0 && s + 1 < name.length) return { scope: name.slice(0, s), value: name.slice(s + 1) };
    return null;
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
            {#each issue.labels as lbl (lbl.name)}
              {@const hex = `#${lbl.color}`}
              {@const rgb = hexToRgb(lbl.color)}
              {@const parts = splitScoped(lbl.name)}
              {#if parts}
                <span class="lbl scoped">
                  <span class="scope" style={`background:${hex};color:${textOn(rgb)}`}>{parts.scope}</span>
                  <span
                    class="value"
                    style={`background:rgba(${rgb[0]},${rgb[1]},${rgb[2]},0.16);color:${hex};border-color:${hex}`}
                  >{parts.value}</span>
                </span>
              {:else}
                <span class="lbl solid" style={`background:${hex};color:${textOn(rgb)}`}>{lbl.name}</span>
              {/if}
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
  .lbl {
    display: inline-flex;
    align-items: stretch;
    font-size: 10px;
    line-height: 1.6;
    border-radius: 10px;
    overflow: hidden;
  }
  .lbl.solid {
    padding: 1px 8px;
  }
  .lbl.scoped .scope {
    padding: 1px 8px;
    font-weight: 600;
  }
  .lbl.scoped .value {
    padding: 1px 8px;
    border: 1px solid;
    border-left: none;
    border-top-right-radius: 10px;
    border-bottom-right-radius: 10px;
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
