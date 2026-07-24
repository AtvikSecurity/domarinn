import { useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * A disclosure section: an uppercase heading that is itself the toggle.
 *
 * This replaces four hand-rolled variants (three collapsible copies of the same
 * chevron header plus one non-collapsible `Section` that looked identical but
 * did nothing when clicked). Having one component is what makes the drawer's
 * sections behave consistently.
 *
 * ## Why the heading wraps the button
 *
 * The header renders as `<h3><button aria-expanded>…</button></h3>`, which is
 * deliberate and load-bearing. Assistive tech and the test suite both query
 * these sections, and they disagree about how:
 *
 * - `getByRole("heading", { name: "Output" })` — needs a real heading element.
 * - `getByRole("button", { name: /History/ })` + `aria-expanded` — needs a
 *   button that owns the section's name.
 *
 * Before this component, Output/Assertions satisfied only the first and
 * Prompt/History only the second. Nesting the button inside the heading
 * satisfies both at once: the heading takes its accessible name from its
 * contents, so both roles resolve to the same label.
 *
 * `title` stays in its own element so an exact-text query (`getByText("Output",
 * { exact: true })`) still matches once `meta` is appended.
 */
export function CollapsibleSection({
  title,
  meta,
  actions,
  defaultOpen = true,
  open: controlledOpen,
  onOpenChange,
  children,
  className,
}: {
  title: ReactNode;
  /** Muted, normal-case suffix inside the toggle — counts, short ids. */
  meta?: ReactNode;
  /**
   * Right-aligned controls rendered OUTSIDE the toggle, so they never leak into
   * its accessible name (and so clicking them doesn't collapse the section).
   */
  actions?: ReactNode;
  defaultOpen?: boolean;
  /** Controlled mode — pair with `onOpenChange`. Used where a query's
   *  `enabled` flag is derived from the open state. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  children: ReactNode;
  className?: string;
}) {
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen);
  const open = controlledOpen ?? uncontrolledOpen;

  const toggle = () => {
    const next = !open;
    if (controlledOpen === undefined) setUncontrolledOpen(next);
    onOpenChange?.(next);
  };

  return (
    <section className={className}>
      <div className="flex items-center gap-2">
        <h3 className="min-w-0 flex-1">
          <button
            type="button"
            aria-expanded={open}
            onClick={toggle}
            className="flex w-full items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              className={cn("shrink-0 transition-transform", open && "rotate-90")}
              aria-hidden
            >
              <path d="M9 6l6 6-6 6" />
            </svg>
            <span>{title}</span>
            {meta ? (
              <span className="truncate font-normal normal-case tracking-normal text-muted/80">
                {meta}
              </span>
            ) : null}
          </button>
        </h3>
        {actions ? <div className="shrink-0">{actions}</div> : null}
      </div>
      {open ? <div className="mt-2">{children}</div> : null}
    </section>
  );
}
