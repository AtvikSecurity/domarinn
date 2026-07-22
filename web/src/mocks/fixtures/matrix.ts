import type { MatrixCell, MatrixColumn, MatrixResponse, MatrixRow } from "@/api";
import { clamp, round2, round4 } from "./rng";
import { RUN_META_BY_ID } from "./runMeta";
import { caseScore, generateCases } from "./cases";

// ---------------------------------------------------------------------------
// Matrix pivot (`GET /runs/{id}/matrix`). Mirrors the server's aggregation
// (crates/domarinn-server/src/storage/matrix.rs): a single first-seen (`idx`
// order) scan of the run's cases pivots into distinct `(provider, prompt)`
// columns and `test_id` rows, each cell collapsing every repeat into status
// counts plus flakiness signals. Columns are always complete; only the test
// rows paginate, over their first-seen `idx` boundary.
// ---------------------------------------------------------------------------

const MATRIX_DEFAULT_LIMIT = 100;
const MATRIX_MAX_LIMIT = 500;

interface CellAcc {
  total: number;
  passed: number;
  failed: number;
  errored: number;
  skipped: number;
  scoreSum: number;
  scoreCount: number;
  outputHashes: Set<string>;
  latencySum: number;
  latencyCount: number;
  costSum: number;
  costAny: boolean;
  /** `(repeat, idx, case_key)` — sorted at finalize time. */
  caseKeys: [number, number, string][];
}

interface RowAcc {
  test_id: string;
  firstSeenIdx: number;
  name: string | null;
  cells: Map<number, CellAcc>;
}

function newCellAcc(): CellAcc {
  return {
    total: 0,
    passed: 0,
    failed: 0,
    errored: 0,
    skipped: 0,
    scoreSum: 0,
    scoreCount: 0,
    outputHashes: new Set(),
    latencySum: 0,
    latencyCount: 0,
    costSum: 0,
    costAny: false,
    caseKeys: [],
  };
}

/** `GET /runs/{id}/matrix` response. Returns `undefined` for an unknown run
 *  (the handler maps that to a 404); a known run whose cases carry a provider
 *  simply yields its columns/rows (a single-provider run collapses to one
 *  column). */
export function buildMatrix(
  runId: string,
  opts: { limit?: number; cursor?: number } = {},
): MatrixResponse | undefined {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return undefined;
  const limit = clamp(Math.floor(opts.limit ?? MATRIX_DEFAULT_LIMIT), 1, MATRIX_MAX_LIMIT);
  const cursor = opts.cursor;

  // Scan in first-seen `idx` order (generation order already is `idx` order,
  // but sort defensively so the pivot never depends on push order).
  const cases = [...generateCases(runId)].sort((a, b) => a.idx - b.idx);

  const columnIndex = new Map<string, number>();
  const columns: MatrixColumn[] = [];
  const rowIndex = new Map<string, number>();
  const rowAccs: RowAcc[] = [];

  for (const c of cases) {
    // Exclude legacy/failed-backfill rows (no provider) and rows without a test
    // identity — they cannot form a matrix cell (mirrors the server's WHERE).
    if (!c.provider_id) continue;
    if (!c.test_id) continue;

    const colKey = `${c.provider_id}\x00${c.prompt_id ?? "\x01null"}`;
    let col = columnIndex.get(colKey);
    if (col === undefined) {
      col = columns.length;
      columnIndex.set(colKey, col);
      columns.push({ provider_id: c.provider_id, prompt_id: c.prompt_id });
    }

    let rowPos = rowIndex.get(c.test_id);
    if (rowPos === undefined) {
      rowPos = rowAccs.length;
      rowIndex.set(c.test_id, rowPos);
      rowAccs.push({ test_id: c.test_id, firstSeenIdx: c.idx, name: null, cells: new Map() });
    }
    const row = rowAccs[rowPos]!;
    if (row.name === null && c.name) row.name = c.name;

    let cell = row.cells.get(col);
    if (!cell) {
      cell = newCellAcc();
      row.cells.set(col, cell);
    }
    cell.total += 1;
    if (c.status === "pass") cell.passed += 1;
    else if (c.status === "fail") cell.failed += 1;
    else if (c.status === "error") cell.errored += 1;
    else cell.skipped += 1;
    const score = caseScore(c);
    cell.scoreSum += score;
    cell.scoreCount += 1;
    cell.outputHashes.add(c.output_hash);
    cell.latencySum += c.latency_ms;
    cell.latencyCount += 1;
    cell.costSum += c.cost_usd;
    cell.costAny = true;
    cell.caseKeys.push([c.repeat, c.idx, c.case_key]);
  }

  const totalColumns = columns.length;

  // Rows are already first-seen ordered; paginate over the first-seen `idx`
  // boundary (the same opaque-cursor style the case list uses).
  const afterCursor = rowAccs.filter(
    (r) => cursor === undefined || r.firstSeenIdx > cursor,
  );
  const page = afterCursor.slice(0, limit);
  const hasMore = afterCursor.length > limit;
  const next_cursor =
    hasMore && page.length > 0 ? String(page[page.length - 1]!.firstSeenIdx) : null;

  const rows: MatrixRow[] = page.map((r) => finalizeRow(r, totalColumns));

  return { run_id: runId, columns, rows, next_cursor };
}

function finalizeRow(row: RowAcc, totalColumns: number): MatrixRow {
  const cells: (MatrixCell | null)[] = Array.from({ length: totalColumns }, () => null);
  for (const [col, acc] of row.cells) cells[col] = finalizeCell(acc);
  return { test_id: row.test_id, name: row.name, cells };
}

function finalizeCell(acc: CellAcc): MatrixCell {
  const caseKeys = [...acc.caseKeys]
    .sort((a, b) => a[0] - b[0] || a[1] - b[1])
    .map(([, , key]) => key);
  return {
    total: acc.total,
    passed: acc.passed,
    failed: acc.failed,
    errored: acc.errored,
    skipped: acc.skipped,
    score_mean: acc.scoreCount > 0 ? round4(acc.scoreSum / acc.scoreCount) : null,
    pass_fraction: round4(acc.passed / acc.total),
    distinct_outputs: acc.outputHashes.size,
    latency_ms_mean: acc.latencyCount > 0 ? round2(acc.latencySum / acc.latencyCount) : null,
    cost_usd: acc.costAny ? round4(acc.costSum) : null,
    case_keys: caseKeys,
  };
}
