import { Fragment, memo, useMemo, useState } from "react";
import type { ReactNode } from "react";
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { cn } from "@/lib/cn";
import { useInView } from "@/lib/useInView";
import { CodeSurface } from "./CodeSurface";
import { splitHighlightedLines } from "./highlightLines";

// Only the languages we register are ever highlighted — keeps the lazy chunk
// small and makes `highlightAuto` deterministic over a known set. The `.hljs-*`
// theme lives in index.css and maps onto the design tokens (no shipped theme).
const LANGUAGES: Record<string, LanguageFn> = {
  json,
  xml,
  javascript,
  typescript,
  python,
  bash,
  yaml,
  markdown,
  sql,
  rust,
};

type LanguageFn = Parameters<typeof hljs.registerLanguage>[1];

for (const [name, fn] of Object.entries(LANGUAGES)) {
  hljs.registerLanguage(name, fn);
}

const SUPPORTED = Object.keys(LANGUAGES);

// Common shorthands map onto the registered language names.
const ALIASES: Record<string, string> = {
  js: "javascript",
  jsx: "javascript",
  ts: "typescript",
  tsx: "typescript",
  py: "python",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  yml: "yaml",
  rs: "rust",
  html: "xml",
  md: "markdown",
};

function resolveLanguage(hint?: string): string | undefined {
  if (!hint) return undefined;
  const key = hint.toLowerCase();
  const resolved = ALIASES[key] ?? key;
  return SUPPORTED.includes(resolved) ? resolved : undefined;
}

/**
 * Above either cap the block renders plain. Highlighting is synchronous, so a
 * multi-thousand-line provider dump would otherwise tokenize on the main thread
 * while the drawer is trying to open.
 */
export const MAX_HIGHLIGHT_LINES = 2000;
export const MAX_HIGHLIGHT_CHARS = 100_000;

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** hljs escapes its own output, so the result is always safe to inject. On the
 *  error path we escape ourselves. */
function highlight(
  code: string,
  hint?: string,
): { html: string; language: string | undefined } {
  const language = resolveLanguage(hint);
  try {
    if (language) return { html: hljs.highlight(code, { language }).value, language };
    // No usable hint: `detect.ts` only reports a language when markdown carried
    // a fence tag, so auto-detection is what keeps most blocks coloured at all.
    const auto = hljs.highlightAuto(code, SUPPORTED);
    return { html: auto.value, language: auto.language };
  } catch {
    return { html: escapeHtml(code), language: undefined };
  }
}

export interface CodeBlockProps {
  /** The raw code to render. */
  code: string;
  /** Language hint (a markdown fence tag, usually). Unknown values fall back to
   *  auto-detection, and the header reports whatever was actually used. */
  language?: string;
  /** Defaults to true once the code has more than one line — a lone `1` in the
   *  gutter is noise. */
  showLineNumbers?: boolean;
  /** Controlled soft wrap. Pair with `onWrapChange`; omit both to let the block
   *  own the state. */
  wrap?: boolean;
  onWrapChange?: (wrap: boolean) => void;
  /** Initial wrap state when uncontrolled. */
  defaultWrap?: boolean;
  /** Cap the body and give it a scrollbar. */
  maxHeight?: string;
  className?: string;
  /** Set false to force the plain path for content that is still being written. */
  highlight?: boolean;
}

/**
 * The canonical code block: a header strip naming the language, a line-number
 * gutter, soft wrap, and copy.
 *
 * Two structural notes, both load-bearing:
 *
 *   - The body is a **CSS grid with one row per logical line** — gutter cell,
 *     then code cell. A wrapped line grows its code cell downward without
 *     shifting the gutter, so line numbers stay anchored however many visual
 *     rows one source line spans. Rendering the gutter as a separate column
 *     alongside a single `<pre>` desynchronises the moment anything wraps.
 *   - That grid is why {@link splitHighlightedLines} exists: highlight.js hands
 *     back one HTML string whose spans straddle newlines, which cannot be cut
 *     into per-row cells without rebalancing.
 *
 * The plain and highlighted paths share `renderRow`, so both emit identical
 * structure and the wrap toggle cannot behave differently between them.
 */
function CodeBlockImpl({
  code,
  language,
  showLineNumbers,
  wrap: wrapProp,
  onWrapChange,
  defaultWrap = true,
  maxHeight,
  className,
  highlight: highlightEnabled = true,
}: CodeBlockProps) {
  const [localWrap, setLocalWrap] = useState(defaultWrap);
  const wrap = wrapProp ?? localWrap;

  function handleWrapChange(next: boolean) {
    if (onWrapChange) onWrapChange(next);
    else setLocalWrap(next);
  }

  // Wait until the block is near the viewport. A cached markdown entry can hold
  // a fenced block every few paragraphs, and tokenizing all of them at mount
  // emits a span per token for content nobody has scrolled to.
  const { ref, inView } = useInView<HTMLDivElement>();

  const plainLines = useMemo(() => code.split("\n"), [code]);
  const tooLarge =
    plainLines.length > MAX_HIGHLIGHT_LINES || code.length > MAX_HIGHLIGHT_CHARS;
  const shouldHighlight = highlightEnabled && !tooLarge && inView;

  const highlighted = useMemo(() => {
    if (!shouldHighlight) return null;
    const { html, language: used } = highlight(code, language);
    return { lines: splitHighlightedLines(html), language: used };
  }, [shouldHighlight, code, language]);

  const showNums = showLineNumbers ?? plainLines.length > 1;
  const label = language ?? highlighted?.language ?? "code";

  const renderRow = (i: number, content: ReactNode) => (
    <Fragment key={i}>
      {showNums ? (
        <span
          aria-hidden="true"
          data-testid="code-block-line-num"
          className="select-none pr-3 pl-3 text-right tabular-nums text-muted/60"
        >
          {i + 1}
        </span>
      ) : null}
      <code
        className={cn(
          "block pr-3",
          showNums ? "" : "pl-3",
          wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre",
        )}
      >
        {content}
      </code>
    </Fragment>
  );

  return (
    <CodeSurface
      ref={ref}
      testId="code-block"
      label={label}
      copyValue={code}
      wrap={wrap}
      onWrapChange={handleWrapChange}
      maxHeight={maxHeight}
      className={className}
      // `hljs` sets the default token colour for anything the grammar left
      // untyped, so the highlighted output blends with the plain path.
      bodyClassName="hljs"
    >
      <pre
        className="m-0 grid"
        style={{ gridTemplateColumns: showNums ? "auto 1fr" : "1fr" }}
      >
        {highlighted
          ? highlighted.lines.map((html, i) =>
              renderRow(
                i,
                // A zero-width space keeps an empty line's row height, so the
                // gutter never collapses against a blank source line.
                html === "" ? (
                  "​"
                ) : (
                  <span dangerouslySetInnerHTML={{ __html: html }} />
                ),
              ),
            )
          : plainLines.map((line, i) => renderRow(i, line === "" ? "​" : line))}
      </pre>
    </CodeSurface>
  );
}

/**
 * Memoized so an unrelated parent re-render — the drawer's resize handle, a
 * neighbouring viewer's toggle — does not re-tokenize every block on screen.
 * Every prop is a primitive or a stable string, so the shallow compare is right.
 */
export const CodeBlock = memo(CodeBlockImpl);

CodeBlock.displayName = "CodeBlock";

export default CodeBlock;
