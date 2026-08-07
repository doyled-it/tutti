<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Tutti wordmark: the mark doubles as the lowercase "u" in "Tutti". Set in
     Fraunces (the display face, shared with Sotto). The final period and the two beat
     dots are amber. The u-mark seating is tuned to Fraunces' metrics; if the face
     changes, re-tune width / height / vertical-align on .umark. -->
<script lang="ts">
  let { size = 22, showNumbers = false }: { size?: number; showNumbers?: boolean } = $props();
</script>

<span class="tutti-wordmark" style="font-size:{size}px" aria-label="Tutti">
  T<svg class="umark" viewBox="0 0 52 120" aria-hidden="true">
    <path class="s" d="M13,26 C3,54 3,80 24,88 C44,95 49,58 46,36" />
    <path class="head" d="M46,22 L39,42 L53,42 Z" />
    <circle class="beat" cx="23" cy="88" r="3.8" />
    <circle class="beat" cx="46" cy="52" r="3.8" />
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
    color: #ffb454;
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
    fill: #ffb454;
  }
  .num {
    fill: #ffb454;
    font-family: "Fraunces", serif;
    font-style: italic;
    font-size: 10px;
  }
</style>
