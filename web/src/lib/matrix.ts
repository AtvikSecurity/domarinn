// Pure derivations over a run's provider × prompt × test matrix. Kept
// dependency-light and side-effect free so both the router-connected RunDetail
// page and unit tests can share them (Task 12's matrix view reuses them too).

import type { CaseStatus, MatrixCell, MatrixColumn, MatrixResponse } from "@/api";

/**
 * The run's distinct providers, in first-seen (column) order. Empty while the
 * matrix is still loading (`m` undefined). A run is "multi-provider" — and
 * therefore shows the provider filter chips + grid column — when this has more
 * than one entry.
 */
export function distinctProviders(m: MatrixResponse | undefined): string[] {
  if (!m) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const c of m.columns) {
    if (!seen.has(c.provider_id)) {
      seen.add(c.provider_id);
      out.push(c.provider_id);
    }
  }
  return out;
}

/**
 * The run's distinct non-null prompts, in first-seen (column) order. Columns
 * with no prompt dimension (`prompt_id === null`) are ignored, so a
 * single-provider run with no prompts yields `[]`. A run shows the prompt
 * filter chips + grid column when this has more than one entry.
 */
export function distinctPrompts(m: MatrixResponse | undefined): string[] {
  if (!m) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const c of m.columns) {
    if (c.prompt_id != null && !seen.has(c.prompt_id)) {
      seen.add(c.prompt_id);
      out.push(c.prompt_id);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Matrix-view cell rendering helpers (Task 12). All pure so the bucket mapping
// is unit-tested directly and the view stays a thin renderer over them.
// ---------------------------------------------------------------------------

/** Pass-fraction band for a repeated cell's background intensity. */
export type CellBucket = "empty" | "low" | "half" | "high" | "full";

/**
 * Bucket a repeated cell's `pass_fraction` into one of five bands:
 * `0 → empty`, `(0, 0.5) → low`, `0.5 → half`, `(0.5, 1) → high`, `1 → full`.
 * Written so out-of-range or `NaN` input still resolves (to `empty`), since the
 * value is server-derived and only trusted to be a number.
 */
export function cellBucket(passFraction: number): CellBucket {
  // `!(x > 0)` catches 0, negatives, and NaN in one guard.
  if (!(passFraction > 0)) return "empty";
  if (passFraction < 0.5) return "low";
  if (passFraction === 0.5) return "half";
  if (passFraction < 1) return "high";
  return "full";
}

/**
 * Literal Tailwind background classes per bucket. Enumerated (not built as
 * `bg-pass/${n}`) so the JIT can see every class — a dynamic string would be
 * purged from the build. Fail-tinted toward 0, pass-tinted toward 1, amber at
 * the midpoint.
 */
export const CELL_BUCKET_CLASS: Record<CellBucket, string> = {
  empty: "bg-fail/30",
  low: "bg-fail/15",
  half: "bg-amber/20",
  high: "bg-pass/15",
  full: "bg-pass/30",
};

/** Convenience: the background class for a cell's pass fraction. */
export function cellBucketClass(passFraction: number): string {
  return CELL_BUCKET_CLASS[cellBucket(passFraction)];
}

/**
 * The single case status of a `total === 1` cell (no repeats to aggregate).
 * Error dominates fail dominates skip; an all-clear cell is a pass.
 */
export function singleCellStatus(cell: MatrixCell): CaseStatus {
  if (cell.errored > 0) return "error";
  if (cell.failed > 0) return "fail";
  if (cell.skipped > 0) return "skip";
  return "pass";
}

/** One rendered matrix column: a `(provider, prompt)` pair and the index of its
 *  cell inside every row's `cells[]`. */
export interface DisplayColumn {
  colIndex: number;
  providerId: string;
  promptId: string | null;
}

/** A prompt-section header group: the prompt and the display columns beneath it. */
export interface ColumnGroup {
  promptId: string | null;
  columns: DisplayColumn[];
}

/**
 * Order the matrix's columns for display. With more than one prompt the columns
 * are regrouped prompt-major (prompt spans on top, providers beneath); with a
 * single prompt (or none) the raw provider-order columns are used as-is. Only
 * `(provider, prompt)` pairs that actually exist as columns are emitted.
 */
export function columnGroups(columns: MatrixColumn[]): ColumnGroup[] {
  const prompts: string[] = [];
  const providers: string[] = [];
  for (const c of columns) {
    if (c.prompt_id != null && !prompts.includes(c.prompt_id)) prompts.push(c.prompt_id);
    if (!providers.includes(c.provider_id)) providers.push(c.provider_id);
  }

  // Single prompt (or a promptless run): one group in raw column order.
  if (prompts.length <= 1) {
    return [
      {
        promptId: columns[0]?.prompt_id ?? null,
        columns: columns.map((c, colIndex) => ({
          colIndex,
          providerId: c.provider_id,
          promptId: c.prompt_id,
        })),
      },
    ];
  }

  // Multi-prompt: prompt-major grouping.
  const groups: ColumnGroup[] = [];
  for (const promptId of prompts) {
    const cols: DisplayColumn[] = [];
    for (const providerId of providers) {
      const colIndex = columns.findIndex(
        (c) => c.provider_id === providerId && c.prompt_id === promptId,
      );
      if (colIndex >= 0) cols.push({ colIndex, providerId, promptId });
    }
    if (cols.length > 0) groups.push({ promptId, columns: cols });
  }
  return groups;
}
