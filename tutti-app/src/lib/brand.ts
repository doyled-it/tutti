// SPDX-License-Identifier: AGPL-3.0-or-later
// Canonical geometry for the Tutti mark ("The Upbeat"), shared by the brand
// components so the path lives in exactly one place in the app code. The static
// design assets in brand/*.svg mirror MARK_* (they feed the icon pipeline and
// docs and cannot import TS); if the mark ever changes, update both this file
// and brand/. The amber beats are coloured with the theme's --accent token by
// the components, never a hardcoded hex.

/** The conductor stroke at standalone / icon proportions (viewBox "24 14 76 90"). */
export const MARK_STROKE = "M42,26 C31,54 31,80 55,88 C77,95 83,58 80,36";
export const MARK_ARROW = "M80,22 L72,42 L88,42 Z";
export const MARK_BEATS = [
  { cx: 54, cy: 88 },
  { cx: 80, cy: 52 },
] as const;
export const MARK_BEAT_R = 5.5;
export const MARK_VIEWBOX = "24 14 76 90";

/**
 * The same mark re-seated to Fraunces' baseline / cap metrics for the wordmark,
 * where it stands in for the lowercase "u" in "Tutti" (viewBox "0 0 52 120").
 * Genuinely distinct geometry, not a copy of MARK_*: the bowl rests on the text
 * baseline and the arrow tops out at cap height.
 */
export const WORDMARK_STROKE = "M13,26 C3,54 3,80 24,88 C44,95 49,58 46,36";
export const WORDMARK_ARROW = "M46,22 L39,42 L53,42 Z";
export const WORDMARK_BEATS = [
  { cx: 23, cy: 88 },
  { cx: 46, cy: 52 },
] as const;
export const WORDMARK_BEAT_R = 3.8;
