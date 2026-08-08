import { Suspense, lazy } from "react";
import type { Output } from "@/api";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { PillButton } from "@/components/ui/PillButton";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import { cn } from "@/lib/cn";
import { detectContent, outputToString } from "./detect";
import type { ContentType } from "./detect";
import { JsonTree } from "./JsonTree";
import { RawText } from "./RawText";
import { setRawMode, setWrap, useOutputPrefs } from "./prefs";

const MarkdownView = lazy(() => import("./MarkdownView"));
const CodeView = lazy(() => import("./CodeView"));

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
  // Soft-wrap only matters when a monospace `<pre>` is actually on screen: the
  // raw view, plain text, or the (rendered-or-raw) code view. The json tree and
  // rendered markdown ignore it.
  const showWrap = !showRendered || type === "code";

  const boxMaxHeight = maxHeight;

  function chooseView(next: "rendered" | "raw") {
    setRawMode(next === "raw");
  }

  function toggleWrap() {
    setWrap(!wrap);
  }

  if (raw === "") {
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
        {showWrap ? (
          <PillButton
            onClick={toggleWrap}
            pressed={wrap}
            size="xs"
            title="Toggle soft wrap"
          >
            Wrap
          </PillButton>
        ) : null}
        <Chip size="xs">{TYPE_LABEL[type]}</Chip>
        <CopyButton value={raw} label="Copy" className="ml-auto" />
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
        <RawText text={raw} wrap={wrap} maxHeight={boxMaxHeight} />
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
  const fallback = <RawText text={raw} wrap={wrap} maxHeight={maxHeight} />;

  if (type === "json") {
    const parsed = parseJson(value, raw);
    if (parsed.ok) {
      // Only becomes its own scrollport when a cap was asked for.
      return (
        <div style={maxHeight ? { maxHeight, overflow: "auto" } : undefined}>
          <JsonTree data={parsed.data} />
        </div>
      );
    }
    return fallback;
  }

  if (type === "markdown") {
    return (
      <Suspense fallback={fallback}>
        <div style={maxHeight ? { maxHeight, overflow: "auto" } : undefined}>
          <MarkdownView markdown={raw} />
        </div>
      </Suspense>
    );
  }

  // code
  return (
    <Suspense fallback={fallback}>
      <CodeView code={raw} langHint={hint} wrap={wrap} maxHeight={maxHeight} />
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
