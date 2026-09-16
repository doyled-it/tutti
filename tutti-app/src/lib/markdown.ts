// SPDX-License-Identifier: AGPL-3.0-or-later
// Shared markdown rendering for the app's chat and detail surfaces. `marked` parses GFM, and
// DOMPurify sanitizes before the result ever touches `{@html}`, so agent- or forge-authored
// markdown cannot inject script into the webview. Extracted from IssueDrawer so the design chat
// and the issue drawer render markdown identically through one sanitized path.

import { marked } from "marked";
import DOMPurify from "dompurify";
import { browser } from "$app/environment";

// Render markdown to sanitized HTML. DOMPurify needs a DOM, which does not exist during
// SvelteKit's static build/prerender, so outside the browser this returns "" rather than risk
// emitting unsanitized HTML.
export function renderMarkdown(md: string): string {
  if (!md) return "";
  const raw = marked.parse(md, { async: false, gfm: true, breaks: true }) as string;
  return browser ? DOMPurify.sanitize(raw) : "";
}
