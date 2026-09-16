// SPDX-License-Identifier: AGPL-3.0-or-later
// A persisted, clamped resizable width for a split pane. The Resizer component reports a
// horizontal pointer delta; this owns the width state, clamps it, and persists it to
// localStorage under `key`, so a split's size survives reloads. `grow: "left"` is for a
// right-docked panel whose handle sits on its left edge (a leftward drag grows it), matching
// RoadmapRail; the default grows rightward, matching the sidebar. Extracted from the
// hand-inlined copies in Sidebar and RoadmapRail so every split shares one implementation.

export interface ResizableOptions {
  key: string;
  min: number;
  max: number;
  default: number;
  grow?: "left" | "right";
}

export class ResizableWidth {
  #min: number;
  #max: number;
  #key: string;
  #grow: "left" | "right";
  width = $state(0);

  constructor(opts: ResizableOptions) {
    this.#min = opts.min;
    this.#max = opts.max;
    this.#key = opts.key;
    this.#grow = opts.grow ?? "right";
    this.width = this.#clamp(opts.default);
    try {
      const stored = localStorage.getItem(opts.key);
      if (stored) {
        const parsed = Number(stored);
        if (Number.isFinite(parsed)) this.width = this.#clamp(parsed);
      }
    } catch {
      // localStorage may be unavailable (private mode, etc.); the default stands.
    }
  }

  #clamp(w: number): number {
    return Math.min(this.#max, Math.max(this.#min, w));
  }

  // Bound method so it can be passed straight to `<Resizer onResize={...}>`.
  onResize = (deltaX: number): void => {
    const signed = this.#grow === "left" ? -deltaX : deltaX;
    this.width = this.#clamp(this.width + signed);
    try {
      localStorage.setItem(this.#key, String(this.width));
    } catch {
      // Persisting is best-effort; the in-memory width still applies for this session.
    }
  };
}
