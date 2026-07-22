<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The shared anatomy of every wizard question: heading, plain-English description, the
     control, then optional example and default hints, and the inline validation error. -->
<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    question,
    description,
    example = null,
    fallback = null,
    note = null,
    error = null,
    children,
  }: {
    question: string;
    description: string;
    example?: string | null;
    /** The value used when the user leaves the control alone. */
    fallback?: string | null;
    /** Extra contextual line, e.g. "Detected from your git remote." */
    note?: string | null;
    error?: string | null;
    children: Snippet;
  } = $props();
</script>

<div class="card">
  <h2>{question}</h2>
  <p class="desc">{description}</p>
  <div class="control">{@render children()}</div>
  {#if error}
    <div class="err">{error}</div>
  {/if}
  {#if note}
    <div class="note">{note}</div>
  {/if}
  {#if example}
    <div class="hint"><span class="lead">Example:</span> {example}</div>
  {/if}
  {#if fallback}
    <div class="hint"><span class="lead">Default:</span> {fallback}</div>
  {/if}
</div>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
    font-weight: 650;
  }
  .desc {
    margin: 0;
    font-size: 13px;
    line-height: 1.55;
    color: var(--text-dim);
  }
  .control {
    margin-top: 4px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .err {
    font-size: 12px;
    color: #ef4444;
  }
  .note,
  .hint {
    font-size: 12px;
    color: var(--text-faint);
  }
  .lead {
    color: var(--text-dim);
    font-weight: 600;
  }
</style>
