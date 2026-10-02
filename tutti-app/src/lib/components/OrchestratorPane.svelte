<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The orchestrator chat: a scrolling transcript plus a compose box. Drives one
     resumable claude -p turn per send; deltas stream in over orchestrator://* events.
     Pure transcript logic lives in $lib/orchestrator.ts. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api } from "$lib/ipc";
  import { renderMarkdown } from "$lib/markdown";
  import {
    appendDelta,
    appendTool,
    appendProposal,
    removeProposalAt,
    dropTrailingEmptyAssistant,
    type ChatMessage,
  } from "$lib/orchestrator";
  import { gateStatus, orchestratorBusy } from "$lib/stores";
  import { autogrow } from "$lib/autogrow";
  import TriageProposalCard from "./TriageProposalCard.svelte";

  // Links inside rendered markdown open in the user's external browser, not the Tauri webview
  // (mirrors DesignPane / IssueDrawer). Applied to each assistant message's {@html} container.
  function externalLinks(node: HTMLElement) {
    const handler = (e: MouseEvent) => {
      const a = (e.target as HTMLElement)?.closest("a");
      const href = a?.getAttribute("href");
      if (a && href) {
        e.preventDefault();
        void openUrl(href);
      }
    };
    node.addEventListener("click", handler);
    node.addEventListener("auxclick", handler);
    return {
      destroy: () => {
        node.removeEventListener("click", handler);
        node.removeEventListener("auxclick", handler);
      },
    };
  }

  // Fired after a triage apply so the host can refresh the board it is showing.
  let { onTriaged = null }: { onTriaged?: (() => void) | null } = $props();

  let messages = $state<ChatMessage[]>([]);
  let draft = $state("");
  let thinking = $state(false);
  let error = $state<string | null>(null);

  let showThinking = $derived.by(() => {
    if (!thinking) return false;
    const last = messages[messages.length - 1];
    return !last || last.role === "user" || (last.kind === "text" && last.text === "");
  });

  onMount(() => {
    (async () => {
      try {
        const t = await api.getTranscript();
        messages = t.messages.map((m) => ({ role: m.role, text: m.text, kind: m.kind ?? "text" }));
      } catch (e) {
        error = String(e);
      }
    })();

    const unlisteners = [
      api.onOrchestratorDelta((text) => {
        messages = appendDelta(messages, text);
      }),
      api.onOrchestratorTool((name) => {
        messages = appendTool(messages, name);
      }),
      api.onOrchestratorDone(() => {
        // Deltas built the transcript; on completion just drop a trailing empty bubble (left
        // by a tool-final turn) so the live view matches what was persisted.
        messages = dropTrailingEmptyAssistant(messages);
        thinking = false;
        orchestratorBusy.set(false);
      }),
      api.onOrchestratorError((msg) => {
        error = msg;
        thinking = false;
        orchestratorBusy.set(false);
      }),
      api.onOrchestratorProposal((p) => {
        messages = appendProposal(messages, p);
      }),
    ];
    return () => {
      unlisteners.forEach((p) => p.then((u) => u()));
      // Defensive: if the pane unmounts mid-turn (e.g. switching to the board section), do
      // not leave the sidebar's switch/add gated forever. The backend single-flight guard
      // still prevents a second concurrent turn.
      orchestratorBusy.set(false);
    };
  });

  async function send() {
    const text = draft.trim();
    if (!text || thinking) return;
    error = null;
    messages = [...messages, { role: "user", text, kind: "text" }];
    draft = "";
    thinking = true;
    orchestratorBusy.set(true);
    try {
      await api.sendOrchestratorMessage(text);
    } catch (e) {
      // The error event also fires; this catch covers a rejected invoke with no event.
      error = String(e);
      thinking = false;
      orchestratorBusy.set(false);
    }
  }

  async function applyGateProposal(index: number, commands: string[]) {
    try {
      const status = await api.applyGate(commands);
      gateStatus.set(status);
      messages = removeProposalAt(messages, index);
    } catch (e) {
      error = String(e);
    }
  }

  function dismissProposal(index: number) {
    messages = removeProposalAt(messages, index);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }
</script>

<div class="pane">
  <div class="transcript">
    <div class="thread">
      {#if messages.length === 0 && !showThinking}
        <div class="optional-note">
          Optional. Design and the Board cover the whole build. Use this to course-correct or add a
          one-off the backlog missed, not as a required step.
        </div>
      {/if}
      {#each messages as m, i}
        <div class="msg {m.role} {m.kind}">
          {#if m.kind === "tool"}
            <span class="tool">ran {m.text}</span>
          {:else if m.kind === "proposal" && m.proposal?.kind === "gate"}
            <!-- `{@const}` rather than a cast: `{#if}` narrows m.proposal for the markup, but
                 not inside a handler closure, which runs later. Binding the narrowed value
                 here makes the compiler carry it into the closure. -->
            {@const gate = m.proposal}
            <div class="proposal">
              <div class="proposal-title">Set the verification gate?</div>
              <ul class="proposal-cmds">
                {#each m.proposal.commands as c}
                  <li><code>{c}</code></li>
                {/each}
              </ul>
              {#if m.proposal.rationale}
                <div class="proposal-why">{m.proposal.rationale}</div>
              {/if}
              <div class="proposal-actions">
                <button
                  class="apply"
                  disabled={m.proposal.commands.length === 0}
                  onclick={() => applyGateProposal(i, gate.commands)}
                >
                  Apply
                </button>
                <button class="dismiss" onclick={() => dismissProposal(i)}>Dismiss</button>
              </div>
            </div>
          {:else if m.kind === "proposal" && m.proposal?.kind === "triage"}
            <TriageProposalCard
              proposal={m.proposal}
              onApplied={onTriaged}
              onDismiss={() => dismissProposal(i)}
            />
          {:else if m.role === "user"}
            <div class="bubble">{m.text}</div>
          {:else}
            <!-- Assistant text renders as sanitized markdown, re-rendered on each streamed
                 delta; a half-open code fence renders as a growing code block until it closes. -->
            <div class="md" use:externalLinks>{@html renderMarkdown(m.text)}</div>
          {/if}
        </div>
      {/each}
      {#if showThinking}
        <div class="msg assistant"><div class="thinking">Thinking...</div></div>
      {/if}
    </div>
  </div>
  {#if error}
    <div class="chat-error">{error}</div>
  {/if}
  <div class="compose-wrap">
    <div class="compose">
      <textarea
        bind:value={draft}
        onkeydown={onKey}
        use:autogrow={{ value: draft }}
        placeholder="Ask about this project, or work out its verification gate..."
        disabled={thinking}></textarea>
      <button onclick={send} disabled={thinking || !draft.trim()}>Send</button>
    </div>
  </div>
</div>

<style>
  .pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .transcript {
    flex: 1;
    overflow-y: auto;
    padding: 20px 16px;
  }
  /* A centered reading column, the convention every serious LLM chat converges on, so a reply
     is not stretched across an ultra-wide pane. */
  .thread {
    max-width: 760px;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .optional-note {
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1.5;
    max-width: 60ch;
    padding: 4px 2px;
  }
  .msg {
    display: flex;
  }
  .msg.user {
    justify-content: flex-end;
  }
  /* User turns stay as a compact, accent-tinted bubble; assistant turns are full-column
     markdown (no bubble), which reads better for long, structured replies. */
  .bubble {
    max-width: 80%;
    padding: 9px 13px;
    border-radius: 12px;
    background: var(--accent-bg);
    border: 1px solid var(--accent-border);
    white-space: pre-wrap;
    font-size: 14px;
    line-height: 1.55;
  }
  .md {
    width: 100%;
    font-size: 14px;
    line-height: 1.62;
    color: var(--text);
    overflow-wrap: anywhere;
  }
  .thinking {
    color: var(--text-faint);
    font-style: italic;
    font-size: 14px;
  }
  .tool {
    font-size: 11px;
    color: var(--text-faint);
    font-family: monospace;
  }
  /* Markdown element styling for an assistant reply. */
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4) {
    line-height: 1.3;
    margin: 1.1em 0 0.5em;
    font-weight: 600;
  }
  .md :global(h1) {
    font-size: 1.4em;
  }
  .md :global(h2) {
    font-size: 1.25em;
  }
  .md :global(h3) {
    font-size: 1.1em;
  }
  .md :global(> :first-child) {
    margin-top: 0;
  }
  .md :global(p),
  .md :global(ul),
  .md :global(ol),
  .md :global(blockquote),
  .md :global(table) {
    margin: 0.6em 0;
  }
  .md :global(ul),
  .md :global(ol) {
    padding-left: 1.4em;
  }
  .md :global(li) {
    margin: 0.2em 0;
  }
  .md :global(a) {
    color: var(--accent);
    text-decoration: underline;
    cursor: pointer;
  }
  .md :global(blockquote) {
    border-left: 3px solid var(--border);
    margin-left: 0;
    padding-left: 12px;
    color: var(--text-dim);
  }
  .md :global(code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.88em;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.1em 0.35em;
  }
  /* Fenced blocks: a dark, rounded, horizontally-scrolling panel; the inline-code chrome is
     reset inside so the block reads as one surface. */
  .md :global(pre) {
    background: var(--code-bg, #0d1117);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px 14px;
    overflow-x: auto;
    margin: 0.7em 0;
  }
  .md :global(pre code) {
    background: none;
    border: none;
    padding: 0;
    font-size: 0.82em;
    line-height: 1.5;
    color: var(--code-fg, #e6edf3);
  }
  .md :global(table) {
    border-collapse: collapse;
    display: block;
    overflow-x: auto;
    max-width: 100%;
  }
  .md :global(th),
  .md :global(td) {
    border: 1px solid var(--border);
    padding: 5px 10px;
    text-align: left;
  }
  .md :global(th) {
    background: var(--bg-panel);
  }
  .md :global(img) {
    max-width: 100%;
  }
  .chat-error {
    font-size: 11px;
    padding: 6px 14px;
    background: rgba(239, 68, 68, 0.12);
    color: #ef4444;
  }
  /* The composer tracks the same centered column width as the thread. */
  .compose-wrap {
    border-top: 1px solid var(--border);
    padding: 12px 16px;
  }
  .compose {
    max-width: 760px;
    margin: 0 auto;
    display: flex;
    gap: 8px;
  }
  .compose textarea {
    flex: 1;
    /* Height is driven by the autogrow action (grows to fit the message up to a cap). */
    resize: none;
    min-height: 44px;
    padding: 8px 10px;
    font: inherit;
    font-size: 13px;
    background: var(--bg-panel);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .compose button {
    align-self: flex-end;
    padding: 8px 16px;
    cursor: pointer;
  }
  .compose button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .proposal {
    max-width: 80%;
    border: 1px solid var(--accent);
    border-radius: 10px;
    padding: 10px 12px;
    background: var(--bg-panel);
    font-size: 13px;
  }
  .proposal-title {
    font-weight: 600;
    margin-bottom: 6px;
  }
  .proposal-cmds {
    margin: 0 0 6px;
    padding-left: 18px;
  }
  .proposal-cmds code {
    font-family: monospace;
    font-size: 12px;
  }
  .proposal-why {
    color: var(--text-dim);
    font-size: 12px;
    margin-bottom: 8px;
  }
  .proposal-actions {
    display: flex;
    gap: 8px;
  }
  .proposal-actions .apply {
    background: var(--accent);
    color: var(--on-accent);
    border: none;
    border-radius: 6px;
    padding: 4px 12px;
    cursor: pointer;
  }
  .proposal-actions .apply:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .proposal-actions .dismiss {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    padding: 4px 12px;
    cursor: pointer;
  }
</style>
