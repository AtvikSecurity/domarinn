// The run-set browser's fixture (`/api/v1/sets*`): aggregates over the same
// generated runs the rest of the mock serves, plus the mutable restriction and
// grant rows the access panel writes.
//
// Mirrors `storage/sets.rs` where it matters:
//   * aggregates are SUMs over every VISIBLE run of the set, never of one run;
//   * a run's pass rate is `pass_count / case_count` (the server's PASS_RATE),
//     not `pass / (pass + fail + error)`;
//   * sparklines are the last 20 runs, oldest first;
//   * `restricted` is COVERING on the browse views (a project row locks its
//     suites) and EXACT on the access payload, whose toggle owns that one row;
//   * a project or suite whose runs are all invisible DISAPPEARS rather than
//     listing with a zero count — which is what makes the 404 on the detail
//     routes indistinguishable from "never existed".

import type {
  GrantLevel,
  ProjectSetDetailResponse,
  ProjectSetView,
  SetAccessResponse,
  SetGrantView,
  SetsResponse,
  SuiteSetDetailResponse,
  SuiteSetView,
} from "@/api";
import { parseTimestamp } from "@/lib/format";
import { allRunSummaries } from "./runStats";
import { BASELINE_BY_SUITE } from "./runMeta";

/** How many recent runs a suite's sparkline covers (`SPARKLINE_RUNS`). */
const SPARKLINE_RUNS = 20;
/** How many per-suite rates a project row carries (`PROJECT_SPARK_CAP`). */
const PROJECT_SPARK_CAP = 20;

/**
 * The caller's access class — the mock's `RunVisibility`. Admins (including the
 * implicit static admin the specs browse as) are `full`; a signed-in non-admin
 * rides its own grants; anonymous callers see unrestricted sets only.
 */
export type SetViewer =
  | { kind: "full" }
  | { kind: "user"; id: string }
  | { kind: "public" };

interface RestrictionRow {
  project: string;
  /** `null` covers every suite in the project. */
  suite: string | null;
}

interface GrantRow {
  project: string;
  suite: string | null;
  user_id: string;
  username: string;
  level: GrantLevel;
  created_at: number;
  created_by: string | null;
}

/** Fixed so seeded rows read the same on every reload (see authState). */
const SEED_TIME = Date.UTC(2026, 5, 2, 9, 30, 0);

/**
 * The seeded policy.
 *
 * `support-bot` is locked at the project level and reachable by the mock's
 * `member` account, which holds `manage` over it — the pair the access panel's
 * non-admin path is exercised against. `search-rerank/ndcg-eval` is locked at
 * the SUITE level inside an otherwise open project, which is the case that
 * separates the covering answer from the exact one.
 */
function seededRestrictions(): RestrictionRow[] {
  return [
    { project: "support-bot", suite: null },
    { project: "search-rerank", suite: "ndcg-eval" },
  ];
}

function seededGrants(): GrantRow[] {
  return [
    {
      project: "support-bot",
      suite: null,
      user_id: "u_member",
      username: "member",
      level: "manage",
      created_at: SEED_TIME,
      created_by: "admin",
    },
    {
      project: "support-bot",
      suite: null,
      user_id: "u_sso",
      username: "sso.only",
      level: "view",
      created_at: SEED_TIME + 60_000,
      created_by: "admin",
    },
    {
      project: "search-rerank",
      suite: "ndcg-eval",
      user_id: "u_member",
      username: "member",
      level: "view",
      created_at: SEED_TIME + 120_000,
      created_by: "admin",
    },
  ];
}

let restrictions = seededRestrictions();
let grants = seededGrants();

/** Test hook: restore the seeded restrictions/grants. */
export function resetSets(): void {
  restrictions = seededRestrictions();
  grants = seededGrants();
}

// --- policy ----------------------------------------------------------------

/** Whether THIS exact row is restricted — what the access panel's toggle owns. */
export function setRestrictedExactly(
  project: string,
  suite: string | null,
): boolean {
  return restrictions.some((r) => r.project === project && r.suite === suite);
}

/** Whether a restriction COVERS the set: its own row, or its project's. */
function coveringRestricted(project: string, suite: string | null): boolean {
  return restrictions.some(
    (r) => r.project === project && (r.suite === null || r.suite === suite),
  );
}

const LEVEL_RANK: Record<GrantLevel, number> = { view: 1, upload: 2, manage: 3 };

/** The strongest grant this user holds over the set, project rows included. */
function coveringLevel(
  userId: string,
  project: string,
  suite: string | null,
): GrantLevel | null {
  let best: GrantLevel | null = null;
  for (const g of grants) {
    if (g.user_id !== userId || g.project !== project) continue;
    if (g.suite !== null && g.suite !== suite) continue;
    if (best === null || LEVEL_RANK[g.level] > LEVEL_RANK[best]) best = g.level;
  }
  return best;
}

/**
 * The level to publish to this caller. `null` for the classes that do not ride
 * grants at all: an admin is never filtered by one, and anonymous/static
 * callers can never hold one.
 */
function myLevel(
  viewer: SetViewer,
  project: string,
  suite: string | null,
): GrantLevel | null {
  return viewer.kind === "user"
    ? coveringLevel(viewer.id, project, suite)
    : null;
}

/** Whether this caller may see the set's runs (`visibility_predicate`). */
function visible(
  viewer: SetViewer,
  project: string,
  suite: string | null,
): boolean {
  if (viewer.kind === "full") return true;
  if (!coveringRestricted(project, suite)) return true;
  return viewer.kind === "user" && coveringLevel(viewer.id, project, suite) !== null;
}

/** Whether this caller may read/edit the set's access list (the set gate). */
export function canManageSet(
  viewer: SetViewer,
  project: string,
  suite: string | null,
): boolean {
  if (viewer.kind === "full") return true;
  if (viewer.kind !== "user") return false;
  return coveringLevel(viewer.id, project, suite) === "manage";
}

// --- aggregates ------------------------------------------------------------

interface SetRun {
  id: string;
  project: string;
  suite: string;
  created_at: number;
  pass_count: number;
  fail_count: number;
  error_count: number;
  case_count: number;
}

let SET_RUNS: SetRun[] | null = null;

/**
 * Every run that carries both a project and a suite, oldest first.
 *
 * Memoized: the corpus is generated (each run's counts come from generating its
 * cases), it never changes, and every handler below walks it — recomputing it
 * per request made the browse endpoints the slowest thing in the mock.
 */
function setRuns(): SetRun[] {
  if (SET_RUNS) return SET_RUNS;
  const out: SetRun[] = [];
  for (const r of allRunSummaries()) {
    // A run with no project can never be restricted (no row can name it) and
    // is not part of any set — the server's `project IS NOT NULL` predicate.
    if (r.project === null || r.suite === null) continue;
    out.push({
      id: r.id,
      project: r.project,
      suite: r.suite,
      created_at: parseTimestamp(r.created_at),
      pass_count: r.pass_count,
      fail_count: r.fail_count,
      error_count: r.error_count,
      case_count: r.case_count,
    });
  }
  out.sort((a, b) => a.created_at - b.created_at || a.id.localeCompare(b.id));
  SET_RUNS = out;
  return out;
}

interface Aggregate {
  run_count: number;
  last_run_at: number | null;
  pass_count: number;
  fail_count: number;
  error_count: number;
  case_count: number;
}

function aggregate(runs: SetRun[]): Aggregate {
  return runs.reduce<Aggregate>(
    (acc, r) => ({
      run_count: acc.run_count + 1,
      last_run_at: Math.max(acc.last_run_at ?? 0, r.created_at),
      pass_count: acc.pass_count + r.pass_count,
      fail_count: acc.fail_count + r.fail_count,
      error_count: acc.error_count + r.error_count,
      case_count: acc.case_count + r.case_count,
    }),
    {
      run_count: 0,
      last_run_at: null,
      pass_count: 0,
      fail_count: 0,
      error_count: 0,
      case_count: 0,
    },
  );
}

/** The server's `PASS_RATE`: a run with no cases is 0.0, never a gap. */
function runPassRate(r: SetRun): number {
  return r.case_count > 0 ? r.pass_count / r.case_count : 0;
}

/** The suite's last 20 visible pass rates, oldest first. */
function sparkline(runs: SetRun[]): number[] {
  return runs.slice(-SPARKLINE_RUNS).map(runPassRate);
}

function suiteRunsOf(
  viewer: SetViewer,
  project: string,
  suite?: string,
): Map<string, SetRun[]> {
  const bySuite = new Map<string, SetRun[]>();
  for (const r of setRuns()) {
    if (r.project !== project) continue;
    if (suite !== undefined && r.suite !== suite) continue;
    if (!visible(viewer, r.project, r.suite)) continue;
    const list = bySuite.get(r.suite) ?? [];
    list.push(r);
    bySuite.set(r.suite, list);
  }
  return bySuite;
}

// --- browse ----------------------------------------------------------------

/** `GET /sets` — every project with at least one visible run, name-ascending. */
export function listRunSets(viewer: SetViewer): SetsResponse {
  const byProject = new Map<string, SetRun[]>();
  for (const r of setRuns()) {
    if (!visible(viewer, r.project, r.suite)) continue;
    const list = byProject.get(r.project) ?? [];
    list.push(r);
    byProject.set(r.project, list);
  }

  const projects: ProjectSetView[] = [...byProject.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([project, runs]) => {
      const agg = aggregate(runs);
      // One latest rate per visible suite, suites in name order, capped.
      const latestBySuite = new Map<string, SetRun>();
      for (const r of runs) {
        const seen = latestBySuite.get(r.suite);
        if (!seen || r.created_at >= seen.created_at) latestBySuite.set(r.suite, r);
      }
      const recent_pass_rates = [...latestBySuite.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([, r]) => runPassRate(r))
        .slice(0, PROJECT_SPARK_CAP);
      return {
        project,
        suite_count: latestBySuite.size,
        run_count: agg.run_count,
        last_run_at: agg.last_run_at,
        pass_count: agg.pass_count,
        fail_count: agg.fail_count,
        error_count: agg.error_count,
        case_count: agg.case_count,
        recent_pass_rates,
        restricted: setRestrictedExactly(project, null),
        my_level: myLevel(viewer, project, null),
      };
    });

  return { projects };
}

/** `GET /sets/{project}` — `null` (a 404) when nothing in it is visible. */
export function runSetProject(
  project: string,
  viewer: SetViewer,
): ProjectSetDetailResponse | null {
  const bySuite = suiteRunsOf(viewer, project);
  if (bySuite.size === 0) return null;

  const suites: SuiteSetView[] = [...bySuite.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([suite, runs]) => {
      const agg = aggregate(runs);
      const series = sparkline(runs);
      return {
        suite,
        run_count: agg.run_count,
        last_run_at: agg.last_run_at,
        pass_count: agg.pass_count,
        fail_count: agg.fail_count,
        error_count: agg.error_count,
        case_count: agg.case_count,
        latest_pass_rate: series[series.length - 1] ?? null,
        sparkline: series,
        baseline_run_id: BASELINE_BY_SUITE.get(`${project}/${suite}`) ?? null,
        // Covering: a locked project locks the suites under it.
        restricted: coveringRestricted(project, suite),
        my_level: myLevel(viewer, project, suite),
      };
    });

  return {
    project,
    restricted: setRestrictedExactly(project, null),
    my_level: myLevel(viewer, project, null),
    suites,
  };
}

/** `GET /sets/{project}/suites/{suite}` — `null` (a 404) when invisible. */
export function runSetSuite(
  project: string,
  suite: string,
  viewer: SetViewer,
): SuiteSetDetailResponse | null {
  const runs = suiteRunsOf(viewer, project, suite).get(suite);
  if (!runs || runs.length === 0) return null;

  const agg = aggregate(runs);
  const series = sparkline(runs);
  return {
    project,
    suite,
    restricted: coveringRestricted(project, suite),
    my_level: myLevel(viewer, project, suite),
    run_count: agg.run_count,
    last_run_at: agg.last_run_at,
    pass_count: agg.pass_count,
    fail_count: agg.fail_count,
    error_count: agg.error_count,
    case_count: agg.case_count,
    latest_pass_rate: series[series.length - 1] ?? null,
    sparkline: series,
    baseline_run_id: BASELINE_BY_SUITE.get(`${project}/${suite}`) ?? null,
  };
}

// --- access ----------------------------------------------------------------

/** `GET /sets/.../access`. Both halves are EXACT scope — see the module head. */
export function setAccess(
  project: string,
  suite: string | null,
): SetAccessResponse {
  const rows: SetGrantView[] = grants
    .filter((g) => g.project === project && g.suite === suite)
    .sort((a, b) => a.created_at - b.created_at || a.username.localeCompare(b.username))
    .map((g) => ({
      user_id: g.user_id,
      username: g.username,
      level: g.level,
      created_at: g.created_at,
      created_by: g.created_by,
    }));
  return { restricted: setRestrictedExactly(project, suite), grants: rows };
}

/** Idempotent, like the server: re-restricting an already-locked set is a no-op. */
export function restrictSet(project: string, suite: string | null): void {
  if (!setRestrictedExactly(project, suite)) restrictions.push({ project, suite });
}

/** `false` — a 404 — when there was no restriction row to remove. */
export function unrestrictSet(project: string, suite: string | null): boolean {
  const before = restrictions.length;
  restrictions = restrictions.filter(
    (r) => !(r.project === project && r.suite === suite),
  );
  return restrictions.length !== before;
}

export function upsertGrant(
  project: string,
  suite: string | null,
  userId: string,
  username: string,
  level: GrantLevel,
  createdBy: string | null,
): void {
  const existing = grants.find(
    (g) => g.project === project && g.suite === suite && g.user_id === userId,
  );
  if (existing) {
    existing.level = level;
    return;
  }
  grants.push({
    project,
    suite,
    user_id: userId,
    username,
    level,
    created_at: Date.now(),
    created_by: createdBy,
  });
}

/** `false` — a 404 — when this user held no grant on the exact set. */
export function deleteGrant(
  project: string,
  suite: string | null,
  userId: string,
): boolean {
  const before = grants.length;
  grants = grants.filter(
    (g) => !(g.project === project && g.suite === suite && g.user_id === userId),
  );
  return grants.length !== before;
}
