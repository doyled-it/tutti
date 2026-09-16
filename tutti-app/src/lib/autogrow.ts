// SPDX-License-Identifier: AGPL-3.0-or-later
// A Svelte action that grows a textarea to fit its content, so the whole message is always
// visible while typing, up to a max height (default 50% of the viewport), after which it scrolls.
// Pass the bound value as `{ value }` so the action re-measures on every change, including a
// programmatic clear after send (the value watch is the single growth trigger, so there is no
// duplicate native input listener). Manual drag-resize is deliberately not offered: auto-fit
// already shows the whole message, and a `resize: vertical` handle would be reset on the next
// keystroke, which is a worse affordance than none.

export interface AutogrowOptions {
  // Max height in CSS pixels. Defaults to half the viewport height at grow time.
  maxPx?: number;
  // The bound text. Passing it makes the `{ value }` argument reactive, so Svelte calls `update`
  // (and re-measures) whenever the text changes, both on typing and on a programmatic reset.
  value?: string;
}

export function autogrow(node: HTMLTextAreaElement, options: AutogrowOptions = {}) {
  let opts = options;

  function maxHeight(): number {
    return opts.maxPx ?? Math.round(window.innerHeight * 0.5);
  }

  function grow() {
    // Measure from a collapsed height so a shrink (deleting lines) is reflected too.
    node.style.height = "auto";
    const cap = maxHeight();
    node.style.height = `${Math.min(node.scrollHeight, cap)}px`;
    node.style.overflowY = node.scrollHeight > cap ? "auto" : "hidden";
  }

  // Initial sizing once the DOM has settled (an empty textarea sizes to its min-height).
  requestAnimationFrame(grow);

  return {
    update(next: AutogrowOptions) {
      opts = next;
      grow();
    },
  };
}
