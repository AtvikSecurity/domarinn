import { useEffect, useState } from "react";

interface Part {
  value: string;
  added?: boolean;
  removed?: boolean;
}

/**
 * Word-level diff of two outputs, rendered side by side. jsdiff is imported
 * lazily so it only ships in the chunk for users who open a compare row.
 */
export function DiffView({ base, head }: { base: string; head: string }) {
  const [parts, setParts] = useState<Part[] | null>(null);

  useEffect(() => {
    let alive = true;
    setParts(null);
    import("diff")
      .then((mod) => {
        if (alive) setParts(mod.diffWords(base ?? "", head ?? ""));
      })
      .catch(() => {
        if (alive) setParts([{ value: head }]);
      });
    return () => {
      alive = false;
    };
  }, [base, head]);

  if (parts === null) {
    return <div className="p-3 text-xs text-muted">Computing diff…</div>;
  }

  return (
    <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
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
