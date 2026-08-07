<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Tutti wordmark: the mark doubles as the lowercase "u" in "Tutti". Set in
     Fraunces (the display face, shared with Sotto). The final period and the two beat
     dots use the theme accent (amber). The u-mark seating is tuned to Fraunces'
     metrics; if the face changes, re-tune width / height / vertical-align on .umark.
     Geometry lives in $lib/brand. Must be plain inline flow (not flex) so the
     vertical-align seating applies. -->
<script lang="ts">
  import { WORDMARK_STROKE, WORDMARK_ARROW, WORDMARK_BEATS, WORDMARK_BEAT_R } from "$lib/brand";

  let { size = 22, showNumbers = false }: { size?: number; showNumbers?: boolean } = $props();
</script>

<span class="tutti-wordmark" style="font-size:{size}px" aria-label="Tutti">
  T<svg class="umark" viewBox="0 0 52 120" aria-hidden="true">
    <path class="s" d={WORDMARK_STROKE} />
    <path class="head" d={WORDMARK_ARROW} />
    {#each WORDMARK_BEATS as beat}
      <circle class="beat" cx={beat.cx} cy={beat.cy} r={WORDMARK_BEAT_R} />
    {/each}
    {#if showNumbers}
      <text class="num" x="19" y="110" text-anchor="middle">1</text>
      <text class="num" x="54" y="55" text-anchor="start">2</text>
    {/if}
  </svg>tti<span class="dot">.</span>
</span>

<style>
  .tutti-wordmark {
    font-family: "Fraunces", Georgia, "Times New Roman", serif;
    font-weight: 500;
    color: var(--text);
    letter-spacing: 0.004em;
    white-space: nowrap;
    display: inline-block;
    line-height: 1;
    user-select: none;
  }
  .dot {
    color: var(--accent);
  }
  .umark {
    display: inline-block;
    overflow: visible;
    width: 0.5em;
    height: 1.16em;
    margin: 0 0.07em 0 -0.03em;
    vertical-align: -0.309em;
  }
  .s {
    fill: none;
    stroke: currentColor;
    stroke-width: 5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .head {
    fill: currentColor;
  }
  .beat {
    fill: var(--accent);
  }
  .num {
    fill: var(--accent);
    font-family: "Fraunces", serif;
    font-style: italic;
    font-size: 10px;
  }
</style>
