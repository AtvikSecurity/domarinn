import { hash, pick, rand } from "./rng";
import { BRANCHES, DAY, NOW, SUITE_DEFS, TAG_POOL, type SuiteDef } from "./suites";

// ---------------------------------------------------------------------------
// Run metadata (counts are derived lazily from generated cases).
// ---------------------------------------------------------------------------

export interface RunMeta {
  id: string;
  suiteKey: string;
  suiteDef: SuiteDef;
  runIndex: number; // 0 = oldest in its suite
  runsInSuite: number;
  created_at: number; // epoch millis (internal only; the wire shape is RFC3339)
  git_branch: string;
  git_commit: string;
  ci_run_url?: string;
  tags: string[];
  caseCount: number;
}

export function suiteKeyOf(def: SuiteDef): string {
  return `${def.project}/${def.suite}`;
}

function buildRunMetas(): RunMeta[] {
  const metas: RunMeta[] = [];
  for (const def of SUITE_DEFS) {
    const suiteKey = suiteKeyOf(def);
    for (let i = 0; i < def.runs; i++) {
      const isLatest = i === def.runs - 1;
      const caseCount = def.matrix
        ? def.matrix.providers.length *
          def.matrix.prompts.length *
          def.matrix.tests *
          def.matrix.repeats
        : def.featured
          ? isLatest
            ? 500
            : 460 + Math.floor(rand(suiteKey, i, "cc") * 40)
          : 40 + Math.floor(rand(suiteKey, i, "cc") * 120);
      const interval = DAY * (0.8 + rand(suiteKey, i, "iv") * 0.6);
      const created_at = Math.round(NOW - (def.runs - 1 - i) * interval);
      const branch = pick(BRANCHES, suiteKey, i, "br");
      const tagCount = 1 + Math.floor(rand(suiteKey, i, "tc") * 2);
      const tags = Array.from(
        new Set(
          Array.from({ length: tagCount }, (_, t) =>
            pick(TAG_POOL, suiteKey, i, "tag", t),
          ),
        ),
      );
      metas.push({
        id: `${def.project}-${def.suite}-${String(i + 1).padStart(2, "0")}`,
        suiteKey,
        suiteDef: def,
        runIndex: i,
        runsInSuite: def.runs,
        created_at,
        git_branch: branch,
        git_commit: hash(suiteKey, i, "sha").toString(16).padStart(8, "0").slice(0, 7),
        ci_run_url:
          rand(suiteKey, i, "ci") > 0.3
            ? `https://ci.example.com/${def.project}/${1000 + i}`
            : undefined,
        tags,
        caseCount,
      });
    }
  }
  return metas;
}

export const RUN_METAS = buildRunMetas();
export const RUN_META_BY_ID = new Map(RUN_METAS.map((m) => [m.id, m]));

/** suiteKey -> run ids oldest..newest */
export const SUITE_RUN_IDS = new Map<string, string[]>();
for (const m of RUN_METAS) {
  const list = SUITE_RUN_IDS.get(m.suiteKey) ?? [];
  list[m.runIndex] = m.id;
  SUITE_RUN_IDS.set(m.suiteKey, list);
}

// Mutable baselines (default: previous run in the series).
export const BASELINE_BY_SUITE = new Map<string, string>();
for (const [suiteKey, ids] of SUITE_RUN_IDS) {
  // Default baseline = previous run in the series (or the sole run). Guard the
  // indexed reads instead of asserting; an empty series simply gets no baseline.
  const baseline = ids.length >= 2 ? ids[ids.length - 2] : ids[0];
  if (baseline !== undefined) BASELINE_BY_SUITE.set(suiteKey, baseline);
}
