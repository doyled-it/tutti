<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Tutti mark ("The Upbeat"): a 2/4 conductor beat pattern. The stroke inherits
     currentColor; the two beats use the theme accent (amber). Beat numbers are omitted
     at logo-bug size. Geometry lives in $lib/brand. -->
<script lang="ts">
  import { MARK_STROKE, MARK_ARROW, MARK_BEATS, MARK_BEAT_R, MARK_VIEWBOX } from "$lib/brand";

  let { size = 20, title = "Tutti" }: { size?: number; title?: string } = $props();
</script>

<svg
  class="tutti-mark"
  viewBox={MARK_VIEWBOX}
  width={size}
  height={(size * 90) / 76}
  role="img"
  aria-label={title}
>
  <path
    d={MARK_STROKE}
    fill="none"
    stroke="currentColor"
    stroke-width="5"
    stroke-linecap="round"
    stroke-linejoin="round"
  />
  <path d={MARK_ARROW} fill="currentColor" />
  {#each MARK_BEATS as beat}
    <circle class="beat" cx={beat.cx} cy={beat.cy} r={MARK_BEAT_R} />
  {/each}
</svg>

<style>
  .tutti-mark {
    display: block;
    overflow: visible;
  }
  /* var() is not valid in an SVG presentation attribute, so colour the beats
     via CSS to keep them theme-aware (amber in dark, deep amber in light). */
  .beat {
    fill: var(--accent);
  }
</style>
