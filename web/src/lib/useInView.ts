import { useEffect, useRef, useState } from "react";

/**
 * Tracks whether an element has come within `rootMargin` of the viewport, so
 * expensive work can wait until it is about to be seen.
 *
 * The case drawer renders one output viewer per prompt message plus one for the
 * output, and a cached entry can carry a long markdown document with a fenced
 * block every few paragraphs. Highlighting all of them at mount emits a span per
 * token for content nobody has scrolled to yet.
 *
 * Notes on the two non-obvious choices:
 *
 *   - The observer uses the **default root** (the viewport) plus a margin, not
 *     the nearest scroll container. That is still correct inside a capped
 *     `maxHeight` box: the observer walks every intervening clip, so a block
 *     scrolled out of an inner scroller reads as not intersecting.
 *   - It **latches on first intersection** and disconnects. Without that, a
 *     block would flip back to the plain path whenever it scrolled away and
 *     re-tokenize on the way back.
 */
export function useInView<T extends Element = HTMLDivElement>(
  rootMargin = "800px 0px",
): { ref: React.RefObject<T | null>; inView: boolean } {
  const ref = useRef<T | null>(null);
  // Whether an IntersectionObserver exists is knowable at first render, so the
  // no-observer fallback is seeded here rather than flipped from inside the
  // effect — a synchronous setState in an effect body costs an extra commit
  // before the content appears, and the lint rule forbids it.
  //
  // This is also what makes the hook transparent under test: jsdom does not
  // implement IntersectionObserver, so `inView` is true from the first render
  // and unit tests see highlighted output with no stub in test/setup.ts.
  const [inView, setInView] = useState(() => typeof IntersectionObserver === "undefined");

  useEffect(() => {
    if (inView) return;
    const el = ref.current;
    if (!el) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setInView(true);
          observer.disconnect();
        }
      },
      { rootMargin },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [inView, rootMargin]);

  return { ref, inView };
}
