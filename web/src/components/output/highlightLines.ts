/**
 * Split highlight.js output into per-line HTML fragments, each independently
 * balanced.
 *
 * highlight.js hands back one HTML string for the whole block, and its spans
 * routinely straddle newlines — a Python docstring, a YAML block scalar, a C
 * block comment are all one token spanning several lines:
 *
 *     <span class="hljs-string">&quot;&quot;&quot;Doc\n    spans lines\n    &quot;&quot;&quot;</span>
 *
 * A plain `html.split("\n")` therefore yields middle lines with an unclosed
 * `<span>` and a final line carrying a stray `</span>`. Dropped into separate
 * grid cells (which is what the line-number gutter does) that bleeds the token
 * colour down the rest of the block.
 *
 * So this walks the markup with a stack of open tags, closes the stack at each
 * newline, and replays it on the next line. Spans nest — `hljs-subst` inside
 * `hljs-string` for a template literal — so it has to be a stack, not a flag.
 *
 * The input is already-escaped markup on its way to `dangerouslySetInnerHTML`;
 * nothing here decodes it, so escaped source can never become live markup.
 */

/** hljs emits only `<span class="…">` / `</span>`; everything else in the
 *  string is escaped text and is carried through untouched. */
const TAG = /<span\b[^>]*>|<\/span>/g;

export function splitHighlightedLines(html: string): string[] {
  const lines: string[] = [];
  /** Opening tags currently in scope, outermost first. */
  const open: string[] = [];
  let line = "";
  /**
   * How many of `open` have actually been emitted on the current line.
   *
   * Tags are written lazily — only once the line has text to put inside them.
   * That is what stops a span which closes right after its final newline from
   * prefixing the next line with an empty `<span></span>` pair.
   */
  let written = 0;

  function openPending() {
    while (written < open.length) line += open[written++];
  }

  function endLine() {
    line += "</span>".repeat(written);
    lines.push(line);
    line = "";
    written = 0;
  }

  function addText(chunk: string) {
    const parts = chunk.split("\n");
    for (let i = 0; i < parts.length; i++) {
      if (i > 0) endLine();
      const part = parts[i]!;
      if (part === "") continue;
      openPending();
      line += part;
    }
  }

  TAG.lastIndex = 0;
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = TAG.exec(html)) !== null) {
    if (match.index > cursor) addText(html.slice(cursor, match.index));
    const tag = match[0];
    if (tag === "</span>") {
      open.pop();
      // Only close what this line actually opened. When the tag was still
      // pending (no text followed it on this line) there is nothing to close,
      // and an unbalanced closer from malformed input pops nothing at all.
      if (written > open.length) {
        line += "</span>";
        written--;
      }
    } else {
      open.push(tag);
    }
    cursor = match.index + tag.length;
  }
  if (cursor < html.length) addText(html.slice(cursor));

  line += "</span>".repeat(written);
  lines.push(line);
  return lines;
}
