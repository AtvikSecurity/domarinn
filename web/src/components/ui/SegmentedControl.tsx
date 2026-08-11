import { useRef } from "react";
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
 * Keyboard behaviour matches the role it claims: a roving tabindex makes the
 * group a single tab stop, and Arrow/Home/End move the selection. Announcing
 * "radio group" and then behaving like N independent buttons is worse than not
 * claiming the role at all.
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
  const groupRef = useRef<HTMLDivElement>(null);

  const enabled = options.filter((o) => !o.disabled);
  // The tab stop follows the selection; if `value` matches nothing selectable,
  // fall back to the first enabled option so the group is never unreachable.
  const activeIndex = enabled.findIndex((o) => o.value === value);
  const tabbableValue = (activeIndex >= 0 ? enabled[activeIndex] : enabled[0])
    ?.value;

  function move(delta: number) {
    if (enabled.length === 0) return;
    const from = activeIndex >= 0 ? activeIndex : 0;
    const next = enabled[(from + delta + enabled.length) % enabled.length];
    if (!next) return;
    onChange(next.value);
    // Follow the selection with DOM focus, as a radio group does.
    groupRef.current
      ?.querySelector<HTMLButtonElement>(`[data-value="${CSS.escape(next.value)}"]`)
      ?.focus();
  }

  function onKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    switch (e.key) {
      case "ArrowRight":
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
        e.preventDefault();
        move(-(activeIndex >= 0 ? activeIndex : 0));
        break;
      case "End":
        e.preventDefault();
        move(enabled.length - 1 - (activeIndex >= 0 ? activeIndex : 0));
        break;
    }
  }

  return (
    <div
      ref={groupRef}
      role="radiogroup"
      aria-label={ariaLabel}
      onKeyDown={onKeyDown}
      // An underline strip rather than a joined pill group, per the Atvik
      // design system's tab treatment: no group chrome at all, and the
      // selection carried by a rule under the active label. Deliberately not
      // `w-full` like the source's own strip — these sit inline in toolbars
      // beside chips and buttons, and stretching them would break every row.
      className={cn("inline-flex items-center gap-1", className)}
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            role="radio"
            data-value={opt.value}
            aria-checked={active}
            aria-pressed={active}
            disabled={opt.disabled}
            tabIndex={opt.value === tabbableValue ? 0 : -1}
            onClick={() => onChange(opt.value)}
            className={cn(
              // The rule is a transparent border on every option, not one added
              // to the active one: colouring an existing border keeps all the
              // labels on the same baseline, where adding a border to the
              // selected option alone would shunt it 2px down as you switch.
              "border-b-2 border-transparent font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              size === "xs" ? "px-1.5 pb-1 text-[11px]" : "px-2 pb-1 text-xs",
              active
                ? "border-info text-fg"
                : "text-muted hover:border-border-strong hover:text-fg",
              opt.disabled &&
                "cursor-not-allowed opacity-40 hover:border-transparent hover:text-muted",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
