import { useCallback, useEffect, useRef, useState } from "react";
import {
  clampWidth,
  DEFAULT_WIDTH,
  DRAWER_WIDTH_KEY,
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
  const dragging = useRef(false);

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize panel"
      aria-valuenow={width}
      aria-valuemin={MIN_WIDTH}
      aria-valuemax={typeof window === "undefined" ? width : window.innerWidth}
      tabIndex={0}
      // A 1px visual seam with a wider invisible hit area: a hairline is the
      // right look and an unusable target.
      className="group absolute inset-y-0 left-0 z-10 w-3 -translate-x-1/2 cursor-col-resize focus-visible:outline-none"
      onDoubleClick={onToggle}
      onPointerDown={(e) => {
        dragging.current = true;
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
        e.currentTarget.releasePointerCapture(e.pointerId);
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
      <div className="mx-auto h-full w-px bg-border transition-colors group-hover:bg-accent group-focus-visible:bg-accent" />
    </div>
  );
}
