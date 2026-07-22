// Content classification for the OutputViewer. Pure, dependency-free, and
// exhaustively unit-tested — every branch here is a heuristic that must not
// false-positive on the plain prose that most provider outputs actually are.

import type { Output } from "@/api";

export type ContentType = "json" | "markdown" | "text";

export interface Detection {
  type: ContentType;
  /** A fenced-code language captured from markdown, when present. */
  langHint?: string;
}

/**
 * Coerce any `Output` (free text, a structured JSON value, or null/undefined)
 * into the raw string a `<pre>`/copy button should show. Structured values are
 * pretty-printed; strings pass through untouched. `Output` is `string | unknown`
 * so this parameter accepts everything the wire can carry.
 */
export function outputToString(value: Output): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    // Circular or otherwise unserializable — fall back without risking a
    // "[object Object]"-via-String lint trap.
    return Object.prototype.toString.call(value);
  }
}

/** True when a string starts with `{`/`[` and parses to an object or array. */
function parsesToJsonContainer(s: string): boolean {
  const t = s.trim();
  if (!(t.startsWith("{") || t.startsWith("["))) return false;
  try {
    const parsed: unknown = JSON.parse(t);
    return parsed !== null && typeof parsed === "object";
  } catch {
    return false;
  }
}

/** Number of lines that look like a bullet or ordered-list item. */
function listItemCount(s: string): number {
  let n = 0;
  for (const line of s.split("\n")) {
    if (/^\s{0,3}([-*+]|\d{1,9}[.)])\s+\S/.test(line)) n++;
  }
  return n;
}

/** A GFM-style table needs a header row of pipes followed by a `---` divider. */
function hasTable(s: string): boolean {
  const lines = s.split("\n");
  for (let i = 0; i < lines.length - 1; i++) {
    const header = lines[i];
    const divider = lines[i + 1];
    if (
      header !== undefined &&
      divider !== undefined &&
      header.includes("|") &&
      /^\s*\|?[\s:|-]*-{3,}[\s:|-]*\|?\s*$/.test(divider) &&
      divider.includes("-")
    ) {
      return true;
    }
  }
  return false;
}

const FENCE_LANG = /(^|\n)```([A-Za-z0-9+#-]+)/;

/** Detect a "strong" markdown signal. Weak/ambiguous signals (a lone dash, a
 *  single asterisk that could be multiplication) are deliberately excluded. */
function detectMarkdown(s: string): { markdown: boolean; langHint?: string } {
  const fence = FENCE_LANG.exec(s);
  const signals =
    /^#{1,6}\s+\S/m.test(s) || // ATX heading
    /(^|\n)```/.test(s) || // fenced code block
    /^>\s+\S/m.test(s) || // blockquote
    /\[[^\]]+\]\([^)\s]+\)/.test(s) || // inline link
    /\*\*[^*\n]+\*\*/.test(s) || // bold
    /(^|[^\w`])`[^`\n]+`([^\w`]|$)/.test(s) || // inline code span
    hasTable(s) ||
    listItemCount(s) >= 2; // two+ list items (a single one is likely prose)
  if (!signals) return { markdown: false };
  return { markdown: true, langHint: fence?.[2] };
}

/**
 * Classify an `Output` as `json`, `markdown`, or `text`. Structured values and
 * JSON-parseable strings are `json`; strings with a strong markdown signal are
 * `markdown`; everything else (including JSON-looking-but-invalid prose) is
 * `text`. `Output` is `string | unknown`, so this accepts strings, structured
 * values, and null/undefined alike.
 */
export function detectContent(value: Output): Detection {
  // A structured (non-string) object/array is unambiguously JSON.
  if (value !== null && value !== undefined && typeof value === "object") {
    return { type: "json" };
  }
  if (typeof value !== "string") return { type: "text" };

  const s = value.trim();
  if (s === "") return { type: "text" };
  if (parsesToJsonContainer(s)) return { type: "json" };

  const md = detectMarkdown(value);
  if (md.markdown) return { type: "markdown", langHint: md.langHint };

  return { type: "text" };
}
