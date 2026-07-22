import { cn } from "@/lib/cn";

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  /** When true the segment is rendered muted and cannot be selected (e.g. the
   *  compare diff's Side/Inline options under the large-output perf guard). */
  disabled?: boolean;
}

/**
 * A small, accessible segmented control (a single-select radio group styled as
 * a joined button group). Radix-free: it exposes `radiogroup`/`radio` roles so
 * assistive tech announces the choice, and reflects the selection with both
 * `aria-checked` and `aria-pressed` for tooling that keys off either.
 *
 * Reused across the UI for two-or-more-way view toggles (Rendered|Raw here,
 * list|matrix and diff-mode in later tasks) — keep the typed `options` API
 * stable.
 */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  ariaLabel,
  size = "sm",
  className,
}: {
  options: readonly SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  ariaLabel?: string;
  size?: "sm" | "xs";
  className?: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className={cn(
        "inline-flex items-center gap-0.5 rounded-md border border-border bg-surface p-0.5",
        className,
      )}
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            aria-checked={active}
            aria-pressed={active}
            disabled={opt.disabled}
            onClick={() => onChange(opt.value)}
            className={cn(
              "rounded font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              size === "xs" ? "px-1.5 py-0.5 text-[11px]" : "px-2 py-0.5 text-xs",
              active
                ? "bg-surface-2 text-fg shadow-sm"
                : "text-muted hover:text-fg",
              opt.disabled && "cursor-not-allowed opacity-40 hover:text-muted",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
