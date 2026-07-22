import type { RunConfigResponse } from "@/api";
import { hash } from "./rng";
import { RUN_META_BY_ID, type RunMeta } from "./runMeta";

// ---------------------------------------------------------------------------
// Config snapshot + digest (migration-3 `runs.config_digest` /
// `config_snapshot`). Runs in a suite share config, so the compare view's
// config-drift signal is off within a suite and on across suites — EXCEPT the
// one suite below, whose latest run bumps its config so the drift badge/panel
// have deterministic fixture data to render. The snapshot is separate data
// from a run's cases: bumping it never perturbs any case status/output.
// ---------------------------------------------------------------------------

/** The single suite whose config drifts within the series (its final run bumps
 *  to config revision 1). Chosen as the featured regression suite so the money
 *  compare pair (regression-11 → regression-12) shows a real drift. */
const CONFIG_DRIFT_SUITE = "checkout-agent/regression";

/** A run's config revision: 0 for every run, except the final run of the one
 *  drift suite, which is 1. This is the only place the fixture's config digest
 *  moves within a suite. */
function configRevision(meta: RunMeta): number {
  if (meta.suiteKey !== CONFIG_DRIFT_SUITE) return 0;
  return meta.runIndex === meta.runsInSuite - 1 ? 1 : 0;
}

/** The run's `config_snapshot` — the eval config it was produced from. Shaped
 *  like a real config document (model, params, prompt, asserts) with enough
 *  nesting for the config-drift diff to exercise scalar changes, a prompt-path
 *  string change, and an added key when the revision bumps. */
export function configSnapshot(runId: string): Record<string, unknown> {
  const meta = RUN_META_BY_ID.get(runId);
  if (!meta) return {};
  const def = meta.suiteDef;
  const rev = configRevision(meta);
  const snapshot: Record<string, unknown> = {
    project: def.project,
    suite: def.suite,
    model: rev === 0 ? "gpt-4o-mini" : "gpt-4o",
    params: {
      temperature: rev === 0 ? 0.2 : 0.7,
      max_tokens: 1024,
      top_p: 1,
    },
    prompt: {
      system:
        rev === 0
          ? "You are a careful checkout assistant. Resolve the request and return a structured JSON decision."
          : "You are a meticulous checkout assistant. Resolve the request, cite the relevant policy, and return a structured JSON decision.",
      template: "User request: {{input}}\nRespond with the decision only.",
    },
    asserts: def.labels.map((kind) => ({ kind })),
  };
  // The bumped revision also adds a guardrails block, so the drift diff carries
  // an "added" path alongside the scalar/prompt changes.
  if (rev === 1) {
    snapshot.guardrails = { pii_filter: true };
  }
  return snapshot;
}

/** Deterministic 32-hex-char digest of a run's config snapshot, `blake3:`-
 *  prefixed to mirror the real server's config digest. Equal snapshots (same
 *  suite + revision) yield equal digests, so drift is exactly digest inequality. */
export function configDigest(runId: string): string {
  const json = JSON.stringify(configSnapshot(runId));
  const parts = ["a", "b", "c", "d"].map((salt) =>
    hash("cfg", salt, json).toString(16).padStart(8, "0"),
  );
  return `blake3:${parts.join("")}`;
}

/** `GET /runs/{id}/config` response: the run's config digest + snapshot. Returns
 *  `undefined` for an unknown run (the handler maps that to a 404). */
export function runConfig(runId: string): RunConfigResponse | undefined {
  if (!RUN_META_BY_ID.has(runId)) return undefined;
  return {
    run_id: runId,
    config_digest: configDigest(runId),
    config: configSnapshot(runId),
  };
}
