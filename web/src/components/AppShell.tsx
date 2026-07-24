import { createContext, useContext, useEffect, useState } from "react";

/**
 * Lets a page take over the viewport instead of scrolling inside the shell.
 *
 * Most pages want ordinary page scrolling. Run detail does not: its case grid
 * must scroll horizontally, and `overflow-x: auto` forces the other axis to a
 * scrollport as well, so the grid is unavoidably its own vertical scroller. If
 * the shell scrolls too, the page ends up with two nested scrollers — the wheel
 * is captured by whichever is under the pointer, and the row count below the
 * grid can only be reached by first moving the pointer off it.
 *
 * A page opts in with `useFillViewport()`, and the shell then stops scrolling
 * and hands its remaining height to that page.
 */
const FillViewportContext = createContext<((fill: boolean) => void) | null>(
  null,
);

export function FillViewportProvider({
  value,
  children,
}: {
  value: (fill: boolean) => void;
  children: React.ReactNode;
}) {
  return (
    <FillViewportContext.Provider value={value}>
      {children}
    </FillViewportContext.Provider>
  );
}

/**
 * Declare that this page fills the viewport and owns its own scrolling. The
 * shell reverts on unmount, so a page that stops using it (or errors out)
 * cannot leave the app unscrollable.
 */
export function useFillViewport(): void {
  const setFill = useContext(FillViewportContext);
  useEffect(() => {
    setFill?.(true);
    return () => setFill?.(false);
  }, [setFill]);
}

/** State hook for the shell itself. */
export function useFillViewportState(): {
  fill: boolean;
  setFill: (fill: boolean) => void;
} {
  const [fill, setFill] = useState(false);
  return { fill, setFill };
}
