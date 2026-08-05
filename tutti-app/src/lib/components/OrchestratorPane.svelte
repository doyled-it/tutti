<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The orchestrator chat: a scrolling transcript plus a compose box. Drives one
     resumable claude -p turn per send; deltas stream in over orchestrator://* events.
     Pure transcript logic lives in $lib/orchestrator.ts. -->
<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "$lib/ipc";
  import {
    appendDelta,
    appendTool,
    appendProposal,
    removeProposalAt,
    dropTrailingEmptyAssistant,
    type ChatMessage,
  } from "$lib/orchestrator";
  import { gateStatus, orchestratorBusy } from "$lib/stores";
  import TriageProposalCard from "./TriageProposalCard.svelte";

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
        {:else}
          <div class="bubble">{m.text}</div>
        {/if}
      </div>
    {/each}
    {#if showThinking}
      <div class="msg assistant"><div class="bubble thinking">Thinking...</div></div>
    {/if}
  </div>
  {#if error}
    <div class="chat-error">{error}</div>
  {/if}
  <div class="compose">
    <textarea
      bind:value={draft}
      onkeydown={onKey}
      placeholder="Ask about this project, or work out its verification gate..."
      disabled={thinking}></textarea>
    <button onclick={send} disabled={thinking || !draft.trim()}>Send</button>
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
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .msg {
    display: flex;
  }
  .msg.user {
    justify-content: flex-end;
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
  .msg.user .bubble {
    background: var(--accent-bg);
    border-color: var(--accent-border);
  }
  .bubble.thinking {
    color: var(--text-faint);
    font-style: italic;
  }
  .tool {
    font-size: 11px;
    color: var(--text-faint);
    font-family: monospace;
  }
  .chat-error {
    font-size: 11px;
    padding: 6px 14px;
    background: rgba(239, 68, 68, 0.12);
    color: #ef4444;
  }
  .compose {
    display: flex;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border);
  }
  .compose textarea {
    flex: 1;
    resize: none;
    min-height: 44px;
    max-height: 160px;
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
    color: #fff;
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
