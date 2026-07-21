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
    // Prevent the drag from starting a text selection on the surrounding UI.
    e.preventDefault();
    // Capture the element now: `e.currentTarget` is nulled once this handler returns, so
    // reading it later (in pointerup) would throw and leave the move listener attached.
    const el = e.currentTarget as HTMLElement;
    dragging = true;
    el.setPointerCapture(e.pointerId);
    let lastX = e.clientX;
    // Suppress text selection and keep the resize cursor for the whole drag.
    const prevUserSelect = document.body.style.userSelect;
    const prevCursor = document.body.style.cursor;
    document.body.style.userSelect = "none";
    document.body.style.cursor = "col-resize";

    function onPointerMove(ev: PointerEvent) {
      const delta = ev.clientX - lastX;
      lastX = ev.clientX;
      onResize(delta);
    }

    function onPointerUp(ev: PointerEvent) {
      dragging = false;
      // Remove the listeners first so the drag always ends, even if releasing the
      // pointer capture throws.
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
      document.body.style.userSelect = prevUserSelect;
      document.body.style.cursor = prevCursor;
      try {
        el.releasePointerCapture(ev.pointerId);
      } catch {
        // The capture may already be gone; nothing to do.
      }
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
