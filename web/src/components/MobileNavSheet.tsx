import { useEffect, useRef, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { NavLink } from "react-router";
import type { NavItem } from "@/lib/nav";
import { cn } from "@/lib/cn";

/** Matches the `md` breakpoint the header strip appears at. */
const DESKTOP_QUERY = "(min-width: 768px)";

const MenuIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    aria-hidden
  >
    <path d="M4 7h16M4 12h16M4 17h16" />
  </svg>
);

const CloseIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    aria-hidden
  >
    <path d="M6 6l12 12M18 6L6 18" />
  </svg>
);

/**
 * The primary navigation, below the `md` breakpoint where the header strip is
 * hidden.
 *
 * Anchored right, because `drawer-in` in index.css animates from `translateX(100%)`
 * and is shared with the two case drawers; a left-hand sheet would need its own
 * keyframe for no gain. Living inside the header's existing `ml-auto shrink-0`
 * cluster also keeps it out of the flex row that the width comment in Layout
 * warns about.
 *
 * The header nav is `hidden md:flex` rather than unmounted, so it is still in
 * the accessibility tree — hence "Main menu" here, not a second "Primary".
 */
export function MobileNavSheet({
  nav,
  children,
}: {
  nav: NavItem[];
  /** Rendered above the links. The search bar, in practice. */
  children?: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);

  // Widening past the breakpoint reveals the strip and hides the trigger, but
  // Radix knows nothing about media queries — the sheet would be left open
  // with a live focus trap and no visible way out. Note the fix is to close
  // it, NOT to hide the content at `md`: that would leave the trap and the
  // scroll lock running behind an invisible panel.
  useEffect(() => {
    if (!open) return;
    const mq = window.matchMedia(DESKTOP_QUERY);
    const onChange = () => {
      if (mq.matches) setOpen(false);
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [open]);

  if (nav.length === 0) return null;

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      {/* `asChild` so Radix owns aria-expanded / aria-controls / aria-haspopup
          on the trigger — hand-written copies would only conflict with them. */}
      <Dialog.Trigger asChild>
        <button
          type="button"
          aria-label="Open menu"
          className="grid size-8 shrink-0 place-items-center rounded-md text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:hidden"
        >
          <MenuIcon />
        </button>
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] data-[state=open]:animate-[overlay-in_120ms_ease-out]" />
        <Dialog.Content
          ref={contentRef}
          tabIndex={-1}
          aria-describedby={undefined}
          // Radix focuses the first tabbable node, which is the search input.
          // On a phone that raises the on-screen keyboard and covers the very
          // list this menu exists to show. Park focus on the panel instead, so
          // the trap and Escape still have an anchor.
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            contentRef.current?.focus();
          }}
          className="fixed inset-y-0 right-0 z-50 flex w-[min(20rem,85vw)] flex-col gap-3 border-l border-border bg-surface p-4 shadow-2xl outline-none data-[state=open]:animate-[drawer-in_160ms_ease-out]"
        >
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-sm font-semibold">Menu</Dialog.Title>
            <Dialog.Close
              aria-label="Close menu"
              className="grid size-8 place-items-center rounded-md text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <CloseIcon />
            </Dialog.Close>
          </div>

          {children}

          <nav
            aria-label="Main menu"
            className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto"
          >
            {nav.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                // Closed on click rather than on a location change: tapping
                // the page you are already on produces no navigation, and the
                // sheet would simply stay open.
                onClick={() => setOpen(false)}
                className={({ isActive }) =>
                  cn(
                    "rounded-md px-3 py-2.5 text-sm font-medium transition-colors",
                    isActive
                      ? "bg-surface-2 text-fg"
                      : "text-muted hover:bg-surface-2 hover:text-fg",
                  )
                }
              >
                {item.label}
              </NavLink>
            ))}
          </nav>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
