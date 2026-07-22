import type { SegmentedOption } from "@/components/ui/SegmentedControl";
import type { DiffMode } from "./DiffView";

/** Above this per-side character count, word diffing (quadratic-ish) is forced
 *  off in favour of the unified line diff. */
export const DIFF_WORD_LIMIT = 50_000;

/** The three diff renderings the segmented control offers, in display order. */
export const DIFF_MODE_OPTIONS: readonly SegmentedOption<DiffMode>[] = [
  { value: "side", label: "Side" },
  { value: "inline", label: "Inline" },
  { value: "lines", label: "Unified" },
];

export interface DiffGuard {
  /** True when either side exceeds `DIFF_WORD_LIMIT`. */
  oversized: boolean;
  /** The mode actually rendered — forced to `lines` when oversized. */
  effectiveMode: DiffMode;
  /** Segmented-control options with the word-diff modes disabled when oversized. */
  options: readonly SegmentedOption<DiffMode>[];
}

/**
 * Perf guard shared by the compare-row expansion and the case-drawer baseline
 * diff: very large outputs force the unified line diff (word diffing is
 * quadratic-ish) and lock out the Side/Inline options.
 */
export function resolveDiffGuard(
  baseText: string,
  headText: string,
  mode: DiffMode,
): DiffGuard {
  const oversized =
    baseText.length > DIFF_WORD_LIMIT || headText.length > DIFF_WORD_LIMIT;
  const effectiveMode: DiffMode = oversized ? "lines" : mode;
  const options = oversized
    ? DIFF_MODE_OPTIONS.map((o) =>
        o.value === "lines" ? o : { ...o, disabled: true },
      )
    : DIFF_MODE_OPTIONS;
  return { oversized, effectiveMode, options };
}
