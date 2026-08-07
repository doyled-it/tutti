<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- The Tutti mark ("The Upbeat"): a 2/4 conductor beat pattern. The stroke inherits
     currentColor; the two beats are amber. Beat numbers are omitted at logo-bug size.
     Single source of truth for the geometry: brand/tutti-icon.svg. -->
<script lang="ts">
  let { size = 20, title = "Tutti" }: { size?: number; title?: string } = $props();
</script>

<svg
  class="tutti-mark"
  viewBox="24 14 76 90"
  width={size}
  height={(size * 90) / 76}
  role="img"
  aria-label={title}
>
  <path
    d="M42,26 C31,54 31,80 55,88 C77,95 83,58 80,36"
    fill="none"
    stroke="currentColor"
    stroke-width="5"
    stroke-linecap="round"
    stroke-linejoin="round"
  />
  <path d="M80,22 L72,42 L88,42 Z" fill="currentColor" />
  <circle cx="54" cy="88" r="5.5" fill="#ffb454" />
  <circle cx="80" cy="52" r="5.5" fill="#ffb454" />
</svg>

<style>
  .tutti-mark {
    display: block;
    overflow: visible;
  }
</style>
