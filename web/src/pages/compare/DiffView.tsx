import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";

interface Part {
  value: string;
  added?: boolean;
  removed?: boolean;
}

/** The three diff renderings the compare row expansion can show. */
export type DiffMode = "side" | "inline" | "lines";

/** `side`/`inline` word-diff; `lines` is a unified line-diff. */
function usesWordDiff(mode: DiffMode): boolean {
  return mode === "side" || mode === "inline";
}

/**
 * Diff of two outputs in one of three modes:
 * - `side` (default): word-level diff rendered as two columns (Base removals
 *   struck through, Head additions tinted green) — the original behaviour.
 * - `inline`: the same word-level diff interleaved into one flowing block.
 * - `lines`: a unified line diff with `+`/`-` gutters.
 *
 * jsdiff is imported lazily so it only ships in the chunk for users who open a
 * compare row. `diffWords`/`diffLines` come from the same lazy module, so the
 * word modes never recompute when the user toggles between Side and Inline —
 * only switching to/from Unified re-diffs.
 */
export function DiffView({
  base,
  head,
  mode = "side",
}: {
  base: string;
  head: string;
  mode?: DiffMode;
}) {
  const [parts, setParts] = useState<Part[] | null>(null);
  const words = usesWordDiff(mode);

  // Reset to the loading state whenever the inputs — or the diff granularity
  // (word vs line) — change, using the "adjusting state when props change"
  // pattern rather than a synchronous effect. Toggling Side<->Inline keeps
  // `words` true, so the memoised parts survive that switch.
  const [prevInputs, setPrevInputs] = useState({ base, head, words });
  if (
    prevInputs.base !== base ||
    prevInputs.head !== head ||
    prevInputs.words !== words
  ) {
    setPrevInputs({ base, head, words });
    setParts(null);
  }

  useEffect(() => {
    let alive = true;
    import("diff")
      .then((mod) => {
        if (!alive) return;
        setParts(
          words
            ? mod.diffWords(base ?? "", head ?? "")
            : mod.diffLines(base ?? "", head ?? ""),
        );
      })
      .catch(() => {
        if (alive) setParts([{ value: head }]);
      });
    return () => {
      alive = false;
    };
  }, [base, head, words]);

  if (parts === null) {
    return <div className="p-3 text-xs text-muted">Computing diff…</div>;
  }

  if (mode === "inline") return <InlineDiff parts={parts} />;
  if (mode === "lines") return <LinesDiff parts={parts} />;

  return (
    <div data-diff-mode="side" className="grid grid-cols-1 gap-3 md:grid-cols-2">
      <DiffColumn title="Base" parts={parts} side="base" />
      <DiffColumn title="Head" parts={parts} side="head" />
    </div>
  );
}

function DiffColumn({
  title,
  parts,
  side,
}: {
  title: string;
  parts: Part[];
  side: "base" | "head";
}) {
  return (
    <div>
      <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted">
        {title}
      </div>
      <pre className="max-h-72 overflow-auto rounded-lg border border-border bg-bg p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words">
        {parts.map((p, i) => {
          if (side === "base" && p.added) return null;
          if (side === "head" && p.removed) return null;
          const changed = side === "base" ? p.removed : p.added;
          if (!changed) return <span key={i}>{p.value}</span>;
          return (
            <span
              key={i}
              className={
                side === "base"
                  ? "rounded bg-fail/15 text-fail line-through decoration-fail/40"
                  : "rounded bg-pass/15 text-pass"
              }
            >
              {p.value}
            </span>
          );
        })}
      </pre>
    </div>
  );
}

/** Word diff interleaved in a single pane: additions tinted green, removals
 *  red with a strikethrough, unchanged text plain. */
function InlineDiff({ parts }: { parts: Part[] }) {
  return (
    <pre
      data-diff-mode="inline"
      className="max-h-72 overflow-auto rounded-lg border border-border bg-bg p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words"
    >
      {parts.map((p, i) => {
        if (p.added)
          return (
            <span key={i} className="rounded bg-pass/15 text-pass">
              {p.value}
            </span>
          );
        if (p.removed)
          return (
            <span
              key={i}
              className="rounded bg-fail/15 text-fail line-through decoration-fail/40"
            >
              {p.value}
            </span>
          );
        return <span key={i}>{p.value}</span>;
      })}
    </pre>
  );
}

/** Split a jsdiff line-chunk's `value` into individual lines, dropping the
 *  single trailing empty string produced by the chunk's terminating newline
 *  (so a `"a\nb\n"` chunk is two lines, not three). */
function splitLines(value: string): string[] {
  const lines = value.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** Unified line diff: one pane, `+`/`-` gutter chars, added lines pass-tinted,
 *  removed lines fail-tinted, unchanged lines plain. Monospace + soft-wrap are
 *  preserved via the same type/whitespace treatment as the `<pre>` panes. */
function LinesDiff({ parts }: { parts: Part[] }) {
  return (
    <div
      data-diff-mode="lines"
      className="max-h-72 overflow-auto rounded-lg border border-border bg-bg font-mono text-xs leading-relaxed"
    >
      {parts.flatMap((p, i) => {
        const gutter = p.added ? "+" : p.removed ? "-" : " ";
        return splitLines(p.value).map((line, j) => (
          <div
            key={`${i}-${j}`}
            className={cn(
              "flex gap-2 px-3 whitespace-pre-wrap break-words",
              p.added && "bg-pass/12 text-pass",
              p.removed && "bg-fail/12 text-fail",
            )}
          >
            <span aria-hidden className="select-none opacity-60">
              {gutter}
            </span>
            <span className="min-w-0 flex-1">{line}</span>
          </div>
        ));
      })}
    </div>
  );
}
