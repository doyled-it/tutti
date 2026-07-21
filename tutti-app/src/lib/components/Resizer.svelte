<!-- SPDX-License-Identifier: AGPL-3.0-or-later -->
<!-- Thin grab strip for resizing an adjacent panel. Drags from the panel's inner edge,
     tracks pointer movement, and reports the delta to the caller (positive delta grows
     the panel to the right; pass a negated delta for a panel that grows leftward). The
     caller owns the width state and clamping so it can persist the result. -->
<script lang="ts">
  let {
    onResize,
    ariaLabel = "Resize panel",
  }: {
    onResize: (deltaX: number) => void;
    ariaLabel?: string;
  } = $props();

  let dragging = $state(false);

  function onPointerDown(e: PointerEvent) {
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    let lastX = e.clientX;

    function onPointerMove(ev: PointerEvent) {
      const delta = ev.clientX - lastX;
      lastX = ev.clientX;
      onResize(delta);
    }

    function onPointerUp(ev: PointerEvent) {
      dragging = false;
      (e.currentTarget as HTMLElement).releasePointerCapture(ev.pointerId);
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
    }

    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
  }
</script>

<div
  class="resizer"
  class:dragging
  role="separator"
  aria-label={ariaLabel}
  aria-orientation="vertical"
  tabindex="-1"
  onpointerdown={onPointerDown}
></div>

<style>
  .resizer {
    flex: none;
    width: 5px;
    cursor: col-resize;
    background: transparent;
  }
  .resizer:hover,
  .resizer.dragging {
    background: var(--accent-border);
  }
</style>
