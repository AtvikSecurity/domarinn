import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * className combiner that resolves Tailwind conflicts by last-wins.
 *
 * Plain `clsx` concatenates, which makes a conditional override silently
 * dependent on stylesheet emission order rather than on argument order — e.g.
 * a selected grid row passing both `hover:bg-surface-2` and
 * `hover:bg-accent/10` got whichever Tailwind happened to emit later. Every
 * primitive in `components/ui/` takes a `className` override, so that hazard
 * multiplies with each one. `twMerge` makes the later argument win, which is
 * what every call site already assumes.
 *
 * See `cn.test.ts` for the class groups this project actually depends on,
 * including the custom `@theme` colour tokens from `index.css` (tailwind-merge
 * has no knowledge of them and classifies them by shape).
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
