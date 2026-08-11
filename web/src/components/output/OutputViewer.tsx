import { Suspense, lazy } from "react";
import type { Output } from "@/api";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { cn } from "@/lib/cn";
import { detectContent, outputToString } from "./detect";
import type { ContentType } from "./detect";
import { JsonTree } from "./JsonTree";
import { RawText } from "./RawText";
import { setRawMode, setWrap, useOutputPrefs } from "./prefs";

const MarkdownView = lazy(() => import("./MarkdownView"));
// Lazy so highlight.js never lands in the main bundle.
const CodeBlock = lazy(() => import("./CodeBlock"));

/** `code` is only reachable via an explicit `contentType`; detection yields
 *  json/markdown/text. */
type ViewType = ContentType | "code";

const TYPE_LABEL: Record<ViewType, string> = {
  json: "json",
  markdown: "markdown",
  text: "text",
  code: "code",
};

/**
 * Content-aware output renderer shared across the app. Auto-detects markdown /
 * JSON / code / text and offers a Rendered|Raw toggle, a soft-wrap toggle, a
 * type chip, and copy. Heavy renderers (markdown, syntax highlighting) are
 * lazy-loaded behind Suspense, with the raw text shown instantly as the
 * fallback so nothing flashes empty.
 */
export function OutputViewer({
  value,
  contentType = "auto",
  langHint,
  maxHeight,
  className,
}: {
  // Documented union kept as the pinned public API. `Output` is `string |
  // unknown` so this collapses to `unknown`; the explicit members are for
  // readers, hence the scoped disable.
  // eslint-disable-next-line @typescript-eslint/no-redundant-type-constituents
  value: Output | string | null | undefined;
  contentType?: "auto" | "markdown" | "json" | "code" | "text";
  langHint?: string;
  /**
   * Cap the content box and give it its own scrollbar. Opt-in: nesting a
   * capped viewer inside an already-scrolling container (the case drawer) puts
   * two scrollbars ~10px apart, and a four-message prompt produced five of them
   * stacked inside one scrolling pane.
   */
  maxHeight?: string;
  className?: string;
}) {
  const raw = outputToString(value);
  const detected = detectContent(value);
  const type: ViewType = contentType === "auto" ? detected.type : contentType;
  const hint = langHint ?? detected.langHint;

  // text has no distinct rendered view — nothing to toggle to.
  const hasRendered = type !== "text";

  // Shared across every mounted viewer, so two on screen never disagree.
  const { raw: rawMode, wrap } = useOutputPrefs();

  const showRendered = hasRendered && !rawMode;

  // Every surface below except rendered markdown is a `CodeSurface`, and each
  // one carries its own type label, wrap toggle and copy button. Leaving ours on
  // screen too would put two of each within a few pixels, both driving the same
  // shared preference. `MarkdownView` has no header of its own — only its fenced
  // blocks do — so the toolbar stays whole for that one case.
  const childHasHeader = !(showRendered && type === "markdown");

  const boxMaxHeight = maxHeight;

  function chooseView(next: "rendered" | "raw") {
    setRawMode(next === "raw");
  }

  if (raw === "") {
    // `RawText` and not a surface: an empty payload has nothing to copy, wrap or
    // label, and it carries the `(empty)` affordance the block has no place for.
    return <RawText text="" wrap={wrap} maxHeight={boxMaxHeight} className={className} />;
  }

  return (
    <div className={cn("space-y-2", className)}>
      <div className="flex flex-wrap items-center gap-2">
        {hasRendered ? (
          <SegmentedControl
            ariaLabel="Output view"
            options={[
              { value: "rendered", label: "Rendered" },
              { value: "raw", label: "Raw" },
            ]}
            value={rawMode ? "raw" : "rendered"}
            onChange={chooseView}
          />
        ) : null}
        {/* Rendered markdown is the one surface with no header of its own, so it
            is the only one that still needs a label and a copy button here.
            There is deliberately no wrap toggle left: every other view owns one,
            and soft wrap means nothing to rendered prose. */}
        {childHasHeader ? null : (
          <>
            <Chip size="xs">{TYPE_LABEL[type]}</Chip>
            <CopyButton value={raw} label="Copy" className="ml-auto" />
          </>
        )}
      </div>

      {showRendered ? (
        <RenderedView
          type={type}
          value={value}
          raw={raw}
          hint={hint}
          wrap={wrap}
          maxHeight={boxMaxHeight}
        />
      ) : (
        <RawBlock type={type} raw={raw} hint={hint} wrap={wrap} maxHeight={boxMaxHeight} />
      )}
    </div>
  );
}

function RenderedView({
  type,
  value,
  raw,
  hint,
  wrap,
  maxHeight,
}: {
  type: ViewType;
  value: Output;
  raw: string;
  hint?: string;
  wrap: boolean;
  maxHeight?: string;
}) {
  if (type === "json") {
    const parsed = parseJson(value, raw);
    // The tree owns its own scrollport through the surface's cap.
    if (parsed.ok) {
      return (
        <JsonTree
          data={parsed.data}
          wrap={wrap}
          onWrapChange={setWrap}
          maxHeight={maxHeight}
        />
      );
    }
    // Detected as json but it does not parse — show the source rather than an
    // empty tree, through the same block the Raw toggle would have given.
    return <RawBlock type={type} raw={raw} hint={hint} wrap={wrap} maxHeight={maxHeight} />;
  }

  if (type === "markdown") {
    return (
      <Suspense fallback={<RawText text={raw} wrap={wrap} maxHeight={maxHeight} />}>
        <div style={maxHeight ? { maxHeight, overflow: "auto" } : undefined}>
          <MarkdownView markdown={raw} />
        </div>
      </Suspense>
    );
  }

  // code
  return <RawBlock type={type} raw={raw} hint={hint} wrap={wrap} maxHeight={maxHeight} />;
}

/**
 * The raw source, through the shared block: syntax-highlighted, numbered, and
 * carrying its own copy and wrap.
 *
 * `RawText` remains the Suspense fallback rather than the destination — it
 * renders instantly with no lazy chunk behind it, so the drawer never flashes
 * empty while highlight.js loads.
 */
function RawBlock({
  type,
  raw,
  hint,
  wrap,
  maxHeight,
}: {
  type: ViewType;
  raw: string;
  hint?: string;
  wrap: boolean;
  maxHeight?: string;
}) {
  // `text` is prose. Auto-detection would colour ordinary words as keywords and
  // label the block with whatever grammar it guessed at — most provider outputs
  // are a sentence or two, and "sql" over an English sentence is worse than no
  // colour at all. Every other type names a grammar we actually know.
  const prose = type === "text";
  const language = prose ? "text" : (hint ?? type);

  return (
    <Suspense fallback={<RawText text={raw} wrap={wrap} maxHeight={maxHeight} />}>
      <CodeBlock
        code={raw}
        language={language}
        highlight={!prose}
        wrap={wrap}
        onWrapChange={setWrap}
        maxHeight={maxHeight}
      />
    </Suspense>
  );
}

function parseJson(
  value: Output,
  raw: string,
): { ok: true; data: unknown } | { ok: false } {
  if (typeof value === "object" && value !== null) return { ok: true, data: value };
  try {
    return { ok: true, data: JSON.parse(raw) };
  } catch {
    return { ok: false };
  }
}
