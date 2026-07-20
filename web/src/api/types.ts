// Hand-written TS mirrors of the Rust DTOs. The Rust side is the source of
// truth; another track will generate these via ts-rs and replace this file.

export type AuthMode = "open" | "protect-writes" | "closed";

export interface Meta {
  name: string;
  version: string;
  auth_mode: AuthMode;
  supported_schema_versions: number[];
}

export type CaseStatus = "pass" | "fail" | "error" | "skip";

export interface RunSummaryRow {
  id: string;
  project: string;
  suite: string;
  created_at: number; // epoch millis
  git_branch?: string;
  git_commit?: string;
  ci_run_url?: string;
  case_count: number;
  pass_count: number;
  fail_count: number;
  error_count: number;
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd?: number;
  duration_ms: number;
  tags: string[];
}

/** A single assertion result on a lean case row. */
export interface CaseAssertLean {
  label: string;
  kind: string;
  passed: boolean;
  score: number;
}

export interface CaseRow {
  case_key: string;
  name?: string;
  tags: string[];
  status: CaseStatus;
  output_preview?: string;
  asserts: CaseAssertLean[];
  prompt_tokens?: number;
  completion_tokens?: number;
  cost_usd?: number;
  latency_ms: number;
}

/** Full per-assert verdict on the detail view. */
export interface CaseAssertDetail {
  label: string;
  kind: string;
  status: CaseStatus;
  score: number;
  weight: number;
  reason: string;
  details?: unknown;
}

export interface CaseDetail extends Omit<CaseRow, "asserts"> {
  rendered_prompt?: string | Record<string, unknown>;
  output?: string | Record<string, unknown>;
  asserts: CaseAssertDetail[];
}

export type CompareDelta =
  | "newly_failing"
  | "newly_passing"
  | "still_failing"
  | "unchanged"
  | "added"
  | "removed";

export interface CompareRow {
  case_key: string;
  name?: string;
  base_status: CaseStatus | null;
  head_status: CaseStatus | null;
  delta: CompareDelta;
  output_changed: boolean;
}

export interface CompareSummary {
  newly_failing: number;
  newly_passing: number;
  still_failing: number;
  output_changed: number;
  added: number;
  removed: number;
}

export interface CompareResult {
  base: RunSummaryRow;
  head: RunSummaryRow;
  summary: CompareSummary;
  cases: CompareRow[];
}

export interface RunDetailResult extends RunSummaryRow {
  assert_labels: string[];
}

export interface RunsResponse {
  runs: RunSummaryRow[];
  next_cursor?: string;
}

export interface CasesResponse {
  cases: CaseRow[];
  next_cursor?: string;
}

export interface ProjectSummary {
  project: string;
  run_count: number;
  suite_count: number;
  last_run_at?: number;
}

export interface ProjectsResponse {
  projects: ProjectSummary[];
}

export interface SuiteSummary {
  suite: string;
  run_count: number;
  baseline_run_id?: string;
  /** Recent pass-rate series (0..1), oldest -> newest, for the sparkline. */
  pass_rate_series: number[];
  last_run_id?: string;
  last_run_at?: number;
}

export interface SuitesResponse {
  project: string;
  suites: SuiteSummary[];
}

export interface CacheStats {
  entries: number;
  total_bytes: number;
  hits: number;
  misses: number;
  oldest_entry_at?: number;
}
