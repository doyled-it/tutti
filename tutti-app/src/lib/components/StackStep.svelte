<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The wizard's stack question: a thin single-select over STACKS, styled like the
     routing/forge radio options it sits alongside. -->
<script lang="ts">
  import { STACKS } from "$lib/wizard";

  let { value = $bindable() }: { value: string } = $props();
</script>

{#each STACKS as opt (opt.id)}
  <label class="opt" class:on={value === opt.id}>
    <input type="radio" bind:group={value} value={opt.id} />
    <span>{opt.label}</span>
  </label>
{/each}

<style>
  .opt {
    display: flex;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }
  .opt.on {
    border-color: var(--accent-border);
    background: var(--accent-bg);
  }
</style>
