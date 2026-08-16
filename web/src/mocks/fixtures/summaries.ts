import type {
  CacheStatsResponse,
  MetaResponse,
  ProjectListItem,
  SuitePoint,
  SuiteSummary,
} from "@/api";
import { round4, toIso } from "./rng";
import { DAY, NOW, RESULT_SCHEMA_VERSION, SUITE_DEFS } from "./suites";
import { pinnedBaseline, RUN_METAS, SUITE_RUN_IDS, suiteKeyOf } from "./runMeta";
import { runStats } from "./runStats";

export function projectSummaries(): ProjectListItem[] {
  const byProject = new Map<string, { runs: number; suites: Set<string>; last: number }>();
  for (const meta of RUN_METAS) {
    const p = byProject.get(meta.suiteDef.project) ?? { runs: 0, suites: new Set<string>(), last: 0 };
    p.runs++;
    p.suites.add(meta.suiteDef.suite);
    p.last = Math.max(p.last, meta.created_at);
    byProject.set(meta.suiteDef.project, p);
  }
  return [...byProject.entries()]
    .map(([project, v]) => ({
      project,
      run_count: v.runs,
      suite_count: v.suites.size,
      last_run_at: toIso(v.last),
    }))
    .sort((a, b) => a.project.localeCompare(b.project));
}

export function suiteSummaries(project: string): SuiteSummary[] {
  const out: SuiteSummary[] = [];
  for (const def of SUITE_DEFS) {
    if (def.project !== project) continue;
    const suiteKey = suiteKeyOf(def);
    const ids = SUITE_RUN_IDS.get(suiteKey) ?? [];
    // SuitePoint.series is newest-first, capped at 20 runs (see the generated
    // type's doc comment).
    const newestFirst = [...ids].reverse().slice(0, 20);
    const series: SuitePoint[] = newestFirst.map((id) => {
      const s = runStats(id);
      const denom = s.pass_count + s.fail_count + s.error_count;
      return {
        run_id: id,
        created_at: s.created_at,
        total: s.case_count,
        passed: s.pass_count,
        pass_rate: denom === 0 ? 0 : round4(s.pass_count / denom),
      };
    });
    const lastId = ids[ids.length - 1];
    out.push({
      suite: def.suite,
      run_count: ids.length,
      last_run_at: lastId ? runStats(lastId).created_at : null,
      baseline_run_id: pinnedBaseline(suiteKey).runId,
      baseline_branch: pinnedBaseline(suiteKey).branch,
      series,
    });
  }
  return out;
}

export const META: MetaResponse = {
  name: "domarinn",
  version: "0.1.0-mock",
  auth_mode: "open",
  setup_required: false,
  sso_providers: [
    {
      name: "google",
      kind: "oidc",
      label: "Google",
      login_url: "/api/v1/auth/oidc/google/start",
    },
    {
      name: "corp",
      kind: "saml",
      label: "Corp SSO",
      login_url: "/api/v1/auth/saml/corp/start",
    },
  ],
  supported_schema_versions: [2, 3],
  result_schema_version: RESULT_SCHEMA_VERSION,
  cache: {
    max_entry_bytes: 10_485_760,
    max_bytes: 5_368_709_120,
    max_age_days: 30,
  },
  // Two tiers, so the demo exercises the switcher rather than the
  // single-tier branch that hides it.
  cache_tiers: [
    { id: "server", label: "Server", search: "fts" },
    { id: "local", label: "Local disk", search: "substring" },
  ],
  // On, so the mock exercises the connected branch of the settings card; the
  // disabled branch is the trivial one.
  mcp_enabled: true,
};

export function cacheStats(): CacheStatsResponse {
  return {
    entries: 4821,
    total_bytes: 268_435_456,
    hits: 19_233,
    misses: 4821,
    // Drained, which is the steady state. The still-indexing branch is
    // exercised by the entries-page fixtures rather than here.
    unindexed: 0,
    oldest_entry_at: toIso(NOW - 37 * DAY),
  };
}
