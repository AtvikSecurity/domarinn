import { cn } from "@/lib/cn";

/**
 * The shared monospace `<pre>` primitive extracted from the several inline
 * copies that used to live in CaseDrawer / DiffView. `wrap` switches between
 * soft-wrapping long lines and horizontal scrolling; `maxHeight` caps the box
 * and lets it scroll vertically.
 */
export function RawText({
  text,
  wrap,
  maxHeight,
  className,
}: {
  text: string;
  wrap: boolean;
  maxHeight?: string;
  className?: string;
}) {
  return (
    <pre
      style={maxHeight ? { maxHeight } : undefined}
      className={cn(
        "overflow-auto rounded-lg border border-border bg-bg p-3 font-mono text-xs leading-relaxed",
        wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre",
        className,
      )}
    >
      {text ? text : <span className="text-muted">(empty)</span>}
    </pre>
  );
}
