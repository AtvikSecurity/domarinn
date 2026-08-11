import { useEffect, useState, type ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { cn } from "@/lib/cn";
import { ErrorState } from "@/components/States";
import { DrawerResizer, useDrawerWidth } from "./DrawerResizer";
import { Skeleton, SkeletonFrame, SkeletonText } from "./Skeleton";

/**
 * The right-side detail drawer: one shell, generic over what it is showing.
 *
 * `CaseDrawer` and `CacheEntryDrawer` were the same Radix `Dialog` twice —
 * identical overlay and content classes, the same `aria-describedby={undefined}`
 * and `onOpenAutoFocus` workarounds, the same resizer placement — diverging only
 * in their close buttons and in which of them had a width toggle. This owns all
 * of that, so a fix lands once and a third drawer starts from parity.
 *
 * It adds three things neither copy had:
 *
 * - **Prev/next stepping.** `↑`/`↓` and a pair of chevrons walk the list without
 *   closing the drawer, with a `3 of 47` position readout. The page supplies the
 *   neighbours (see `lib/listNeighbors`), so stepping stays a URL edit and every
 *   position remains deep-linkable.
 * - **An eager open.** The shell opens on the selection, not on the data, and
 *   fills with a skeleton until the item lands. The alternative — waiting for
 *   the fetch before rendering anything — makes the click look dead for as long
 *   as the request takes.
 * - **Latching.** The last non-null `item` stays rendered while the next one
 *   loads, so stepping through cases does not flash a skeleton between each
 *   pair. An `error` drops the latch for the whole panel — header, title,
 *   subheader and body — because a failure belongs to the row that was
 *   selected, and framing it with the previous row's identity would say that
 *   row failed instead.
 *
 * Everything in the header describes what is on screen; only the prev/next
 * readout describes where you are going. That is why `renderEyebrow` and
 * `renderHeaderActions` take the shown item rather than closing over the
 * selection: during a step the two disagree, and a "copy this" button that
 * quietly acts on the row you cannot see yet is worse than one that lags.
 *
 * ## What it deliberately does not do
 *
 * Radix owns the focus trap, the scroll lock, Esc, and the portal. This adds
 * only `↑`/`↓` on top. Nothing here hand-rolls a dialog.
 *
 * `open` is a separate prop rather than being derived from the item. It has to
 * be: the drawer must stay open through a failed load, when there is no item at
 * all. There is deliberately no `loading` prop either — open, no error and
 * nothing to show already means "pending", and a second flag saying so could
 * only ever disagree with the first.
 */

export interface DetailDrawerProps<T> {
  /** Whether the drawer is open. Drive from the selection, not from the query. */
  open: boolean;
  /** The loaded item. Null/undefined while pending or failed. */
  item: T | null | undefined;
  /** A failed load. Takes precedence over both the item and the skeleton. */
  error?: unknown;
  /** Retry handler for the error state. */
  onRetry?: () => void;
  /** Close handler — the overlay, Esc, and the ✕ all call it. */
  onClose: () => void;

  /** Step to the previous item. Omit to disable the chevron and `↑`. */
  onPrev?: () => void;
  /** Step to the next item. Omit to disable the chevron and `↓`. */
  onNext?: () => void;
  /** 1-based `{ index, total }` readout between the chevrons. */
  position?: { index: number; total: number };

  /**
   * The dialog's accessible name while pending, and the noun in the control
   * labels: "Previous case (↑)", "Close case drawer", "Loading case…". Once the
   * item lands, `renderTitle` becomes the accessible name.
   */
  navItemLabel: string;

  /**
   * Mono eyebrow line above the title — an id, a key, a short path.
   *
   * Called with `null` when there is nothing to show, because the selection
   * usually carries an identity of its own: the case key and the entry hash are
   * both already in the URL. A caller that can name what it is fetching should,
   * rather than leave a cold open or a failure anonymous.
   */
  renderEyebrow?: (item: T | null) => ReactNode;
  /** The title. Becomes the dialog's accessible name. */
  renderTitle: (item: T) => ReactNode;
  /** Inline accessory on the title line, e.g. a status badge. */
  renderTitleAccessory?: (item: T) => ReactNode;
  /**
   * A band between the header and the scrolling body — a verdict strip, a chip
   * row. It does NOT scroll, which is the point: these are facts you want while
   * reading the body, not facts you scroll past to reach it. It owns its own
   * padding and borders.
   */
  renderSubheader?: (item: T) => ReactNode;
  /**
   * Header buttons, before the width toggle and close.
   *
   * Given the shown item rather than the selection, so a "copy this" button
   * copies what is on screen. During a step the two disagree for as long as the
   * fetch takes, and a control that acts on the row you cannot see yet hands
   * back the wrong id without ever looking wrong.
   */
  renderHeaderActions?: (item: T | null) => ReactNode;
  /**
   * The scrolling body. Owns its own padding — the two drawers disagree about
   * whether the body is one scroller or several blocks, so the shell does not
   * impose a box.
   */
  renderBody: (item: T) => ReactNode;
}

export function DetailDrawer<T>({
  open,
  item,
  error,
  onRetry,
  onClose,
  onPrev,
  onNext,
  position,
  navItemLabel,
  renderEyebrow,
  renderTitle,
  renderTitleAccessory,
  renderSubheader,
  renderHeaderActions,
  renderBody,
}: DetailDrawerProps<T>) {
  const drawer = useDrawerWidth();

  // The last non-null item, so stepping does not blank the body between
  // fetches. Adjusted during render — the documented React pattern for deriving
  // state from props — so nothing stale is ever painted. Both guards compare
  // against a prop, so each converges in one extra pass even when the parent
  // hands us a fresh object identity every render.
  const [held, setHeld] = useState<T | null>(item ?? null);
  // Both branches are gated on `open` so they can never both fire: a closed
  // drawer whose parent still holds an item would otherwise latch and clear on
  // every render, forever.
  if (open && item != null && held !== item) setHeld(item);
  // Drop the latch on close, or the next open would flash the previous
  // selection's content before its own fetch lands.
  if (!open && held !== null) setHeld(null);

  // Precedence: a failure is information the user has to see, so it outranks
  // the latch; real content outranks the skeleton.
  //
  // The latch is dropped for the *whole* panel, not just the body. A failure
  // describes the row that was selected, so keeping the previous row's key,
  // title and subheader above it would read as that row having failed — a
  // different claim, and a false one.
  const failed = error != null;
  const shown = failed ? null : (item ?? held);
  const pending = !failed && shown == null;

  // The caller usually knows the selection's identity without the item — both
  // drawers already have it from the URL — so the eyebrow survives a cold open
  // and a failure. The skeleton is only for callers that cannot say.
  const eyebrow = renderEyebrow?.(shown);

  // `↑`/`↓` on the window rather than on the panel: the shortcut should work
  // wherever focus happens to be inside the drawer. Esc is Radix's.
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      // Never hijack typing. The drawer contains real inputs — the cache
      // drawer's filters, any search box a body renders.
      const target = e.target as HTMLElement | null;
      const tag = target?.tagName;
      if (
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        target?.isContentEditable
      ) {
        return;
      }
      // A control inside the drawer that already acted on this key owns it —
      // the segmented controls in both bodies move their selection with
      // `↑`/`↓`. Stepping the list as well would move two things at once.
      if (e.defaultPrevented) return;
      if (e.key === "ArrowDown" && onNext) {
        e.preventDefault();
        onNext();
      } else if (e.key === "ArrowUp" && onPrev) {
        e.preventDefault();
        onPrev();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onPrev, onNext]);

  const showNav = !!onPrev || !!onNext || !!position;

  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay
          className={cn(
            "detail-drawer-overlay fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px]",
            "data-[state=open]:animate-[overlay-in_120ms_ease-out]",
            "data-[state=closed]:animate-[overlay-out_220ms_ease]",
          )}
        />
        <Dialog.Content
          className={cn(
            "detail-drawer-panel fixed inset-y-0 right-0 z-50 flex max-w-full flex-col",
            "border-l border-border bg-surface shadow-2xl outline-none",
            "data-[state=open]:animate-[drawer-in_160ms_ease-out]",
            "data-[state=closed]:animate-[drawer-out_220ms_cubic-bezier(0.2,0.9,0.2,1)]",
          )}
          style={{ width: drawer.width }}
          aria-describedby={undefined}
          aria-busy={pending}
          // Radix focuses the first tabbable node on open; without this that is
          // the resize handle, which is a confusing place to land.
          onOpenAutoFocus={(e) => e.preventDefault()}
        >
          <DrawerResizer
            width={drawer.width}
            onResize={drawer.set}
            onToggle={drawer.toggle}
          />

          <header className="shrink-0 border-b border-border px-5 py-3">
            <div className="flex items-center gap-2">
              <div className="min-w-0 flex-1 truncate font-mono text-xs text-muted">
                {eyebrow ?? (pending ? <Skeleton className="h-3 w-40" /> : null)}
              </div>

              {showNav ? (
                <div className="flex shrink-0 items-center gap-0.5">
                  <NavButton
                    onClick={onPrev}
                    label={`Previous ${navItemLabel} (↑)`}
                    direction="up"
                  />
                  {position ? (
                    <span className="px-1 font-mono text-xs tabular-nums text-muted">
                      {position.index} of {position.total}
                    </span>
                  ) : null}
                  <NavButton
                    onClick={onNext}
                    label={`Next ${navItemLabel} (↓)`}
                    direction="down"
                  />
                </div>
              ) : null}

              <div className="flex shrink-0 items-center gap-1">
                {renderHeaderActions?.(shown)}
                {/* The drag handle is a hairline and nobody finds it. This is
                    the discoverable path to the same width, and the one that
                    reaches full width in a single click. */}
                <button
                  type="button"
                  onClick={drawer.toggle}
                  aria-label="Toggle panel width"
                  title="Expand / collapse (or drag the left edge)"
                  className={ICON_BUTTON}
                >
                  <svg {...ICON_SVG}>
                    <path d="M9 4l-5 8 5 8M15 4l5 8-5 8" />
                  </svg>
                </button>
                <Dialog.Close
                  className={ICON_BUTTON}
                  aria-label={`Close ${navItemLabel} drawer`}
                >
                  <svg {...ICON_SVG}>
                    <path d="M6 6l12 12M18 6L6 18" />
                  </svg>
                </Dialog.Close>
              </div>
            </div>

            {/* Radix takes the dialog's accessible name from the title, so it is
                never absent and never "undefined" — while pending it is the
                noun, read aloud but not painted, with a bar in its place. */}
            {shown == null ? (
              <>
                <Dialog.Title className="sr-only">{navItemLabel}</Dialog.Title>
                {/* No bar under a failure: a placeholder that never resolves
                    reads as "still loading" beside an error that says it
                    stopped. */}
                {pending ? <Skeleton className="mt-1 h-5 w-1/2" /> : null}
              </>
            ) : (
              <div className="mt-0.5 flex flex-wrap items-center gap-2">
                <Dialog.Title className="min-w-0 truncate text-sm font-semibold">
                  {renderTitle(shown)}
                </Dialog.Title>
                {renderTitleAccessory?.(shown)}
              </div>
            )}
          </header>

          {shown != null ? renderSubheader?.(shown) : null}

          {failed ? (
            <div className="flex-1 overflow-y-auto px-5 py-4">
              <ErrorState error={error} onRetry={onRetry} />
            </div>
          ) : shown != null ? (
            renderBody(shown)
          ) : (
            <DrawerSkeletonBody label={navItemLabel} />
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Shared by the width toggle and the close button. */
const ICON_BUTTON =
  "rounded-md p-1.5 text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

const ICON_SVG = {
  width: 18,
  height: 18,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 2,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
} as const;

/**
 * A prev/next chevron.
 *
 * Up/down rather than left/right: the list it steps through is vertical and the
 * keys bound to it are `↑`/`↓`, so a sideways arrow would contradict both.
 *
 * Rendered disabled rather than hidden when there is no handler — at the ends of
 * the list the control keeps its place instead of the row reflowing under the
 * cursor mid-step.
 */
function NavButton({
  onClick,
  label,
  direction,
}: {
  onClick?: () => void;
  label: string;
  direction: "up" | "down";
}) {
  return (
    <button
      type="button"
      disabled={!onClick}
      onClick={onClick}
      aria-label={label}
      title={label}
      className={cn(
        "rounded-md p-1 text-muted",
        "hover:bg-surface-2 hover:text-fg",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        "disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted",
      )}
    >
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        <path d={direction === "up" ? "M18 15l-6-6-6 6" : "M6 9l6 6 6-6"} />
      </svg>
    </button>
  );
}

/**
 * The body placeholder.
 *
 * Deliberately generic — the shell cannot know any drawer's layout — so it
 * suggests "a few sections of text" rather than mimicking one consumer and
 * being wrong for the other.
 */
function DrawerSkeletonBody({ label }: { label: string }) {
  return (
    <SkeletonFrame label={label} className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
      {[0, 1, 2].map((section) => (
        <div key={section} className="space-y-2">
          <Skeleton className="h-3 w-28" />
          <SkeletonText lines={3} />
        </div>
      ))}
    </SkeletonFrame>
  );
}
