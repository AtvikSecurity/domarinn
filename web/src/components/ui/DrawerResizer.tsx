import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import {
  clampWidth,
  DEFAULT_WIDTH,
  DRAWER_WIDTH_KEY,
  maxWidth,
  MIN_WIDTH,
  parseStoredWidth,
  RESIZE_STEP,
  RESIZE_STEP_LARGE,
  toggledWidth,
} from "@/lib/drawerWidth";

/**
 * The drawer's width, persisted across cases and sessions.
 *
 * Re-clamped on viewport resize: a width chosen on an external monitor would
 * otherwise push the drawer off a laptop screen with no way to grab its edge.
 */
export function useDrawerWidth() {
  const [width, setWidth] = useState<number>(() => {
    if (typeof window === "undefined") return DEFAULT_WIDTH;
    return clampWidth(
      parseStoredWidth(window.localStorage.getItem(DRAWER_WIDTH_KEY)),
      window.innerWidth,
    );
  });

  useEffect(() => {
    try {
      window.localStorage.setItem(DRAWER_WIDTH_KEY, String(width));
    } catch {
      // A full or blocked quota must not break the drawer; the width simply
      // does not persist.
    }
  }, [width]);

  useEffect(() => {
    const onResize = () => setWidth((w) => clampWidth(w, window.innerWidth));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const set = useCallback(
    (next: number) => setWidth(clampWidth(next, window.innerWidth)),
    [],
  );
  const toggle = useCallback(
    () => setWidth((w) => toggledWidth(w, window.innerWidth)),
    [],
  );

  return { width, set, toggle };
}

/**
 * The drag handle on the drawer's leading edge.
 *
 * A `separator` with `aria-valuenow`, following the WAI-ARIA window-splitter
 * pattern, and it is focusable and arrow-key operable — a drag-only affordance
 * would put the drawer's width out of reach for anyone not using a mouse.
 *
 * Pointer capture rather than window listeners: the drag keeps tracking when
 * the cursor leaves the handle, which is exactly what happens when you throw it
 * toward the edge of the screen.
 */
export function DrawerResizer({
  width,
  onResize,
  onToggle,
}: {
  width: number;
  onResize: (next: number) => void;
  onToggle: () => void;
}) {
  // The ref gates pointermove synchronously; state exposes the same lifecycle to
  // the grip and drawer edge so the active affordance stays painted while the
  // pointer is away from this narrow rail.
  const dragging = useRef(false);
  const [isDragging, setIsDragging] = useState(false);

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize panel"
      aria-valuenow={width}
      aria-valuemin={MIN_WIDTH}
      // The clamp, not the viewport: the drawer stops at 95%, and announcing a
      // width the handle refuses to reach makes the control look broken to
      // anyone driving it by the numbers.
      aria-valuemax={
        typeof window === "undefined" ? width : maxWidth(window.innerWidth)
      }
      tabIndex={0}
      // The rail stays wider than its painted tab: a large invisible target is
      // easy to grab without drawing a second line beside the drawer border.
      // `touch-none`: without it the browser may claim the drag as a pan and
      // fire pointercancel part-way through, leaving the drawer at whatever
      // width the gesture had reached.
      className="drawer-resizer group absolute inset-y-0 left-0 z-10 w-3 -translate-x-1/2 cursor-col-resize touch-none select-none focus-visible:outline-none"
      data-dragging={isDragging ? "true" : undefined}
      onDoubleClick={onToggle}
      onPointerDown={(e) => {
        dragging.current = true;
        setIsDragging(true);
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        if (!dragging.current) return;
        // The drawer is right-anchored, so its width is the distance from the
        // pointer to the right edge of the viewport.
        onResize(window.innerWidth - e.clientX);
      }}
      onPointerUp={(e) => {
        dragging.current = false;
        setIsDragging(false);
        e.currentTarget.releasePointerCapture(e.pointerId);
      }}
      onPointerCancel={() => {
        dragging.current = false;
        setIsDragging(false);
      }}
      onKeyDown={(e) => {
        const step = e.shiftKey ? RESIZE_STEP_LARGE : RESIZE_STEP;
        // Left grows the drawer: it is right-anchored, so its edge moves left
        // as it widens.
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          onResize(width + step);
        } else if (e.key === "ArrowRight") {
          e.preventDefault();
          onResize(width - step);
        } else if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onToggle();
        }
      }}
    >
      {/* A solid tab hanging off the leading edge: unlike a hairline, this is
          visible before the user already knows the panel can be resized. */}
      <span
        aria-hidden
        data-testid="drawer-resize-grip"
        className={cn(
          "absolute right-1/2 top-1/2 z-10 flex h-24 w-7 -translate-y-1/2 items-center justify-center",
          "cursor-col-resize rounded-md border border-border bg-surface-2 text-muted shadow-sm",
          "transition-all duration-150",
          "group-hover:scale-110 group-hover:border-accent/60 group-hover:text-fg",
          "group-focus-visible:scale-110 group-focus-visible:border-accent/60 group-focus-visible:text-fg",
          isDragging && "scale-110 border-accent bg-surface-2 text-fg",
        )}
      >
        <svg
          width="20"
          height="20"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <circle cx="9" cy="7" r="0.75" fill="currentColor" stroke="none" />
          <circle cx="15" cy="7" r="0.75" fill="currentColor" stroke="none" />
          <circle cx="9" cy="12" r="0.75" fill="currentColor" stroke="none" />
          <circle cx="15" cy="12" r="0.75" fill="currentColor" stroke="none" />
          <circle cx="9" cy="17" r="0.75" fill="currentColor" stroke="none" />
          <circle cx="15" cy="17" r="0.75" fill="currentColor" stroke="none" />
        </svg>
      </span>
    </div>
  );
}
