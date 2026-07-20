// Fetch-level mock. When enabled, apiRequest routes here instead of the
// network, so the whole app (and tests) runs against the fixture dataset with
// no service worker and no backend.

import type { CaseRow, RunSummaryRow } from "@/api/types";
import * as fx from "./fixtures";

let MOCK_FORCED: boolean | null = null;

/** Test hook: force mock mode on/off regardless of the env var. */
export function setMockEnabled(value: boolean | null): void {
  MOCK_FORCED = value;
}

export function isMockEnabled(): boolean {
  if (MOCK_FORCED !== null) return MOCK_FORCED;
  const v = import.meta.env.VITE_MOCK;
  return v === "1" || v === "true";
}

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function notFound(): Response {
  return json({ error: "not_found" }, 404);
}

const DEFAULT_LIMIT = 100;

function paginate<T>(
  items: T[],
  params: URLSearchParams,
): { page: T[]; next_cursor?: string } {
  const limit = Math.max(1, Number(params.get("limit") ?? DEFAULT_LIMIT));
  const cursor = Number(params.get("cursor") ?? 0) || 0;
  const page = items.slice(cursor, cursor + limit);
  const next = cursor + limit;
  return { page, next_cursor: next < items.length ? String(next) : undefined };
}

function derivedRunStatus(r: RunSummaryRow): string {
  if (r.error_count > 0) return "error";
  if (r.fail_count > 0) return "fail";
  return "pass";
}

function filterRuns(runs: RunSummaryRow[], p: URLSearchParams): RunSummaryRow[] {
  const project = p.get("project");
  const suite = p.get("suite");
  const tag = p.get("tag");
  const branch = p.get("branch");
  const since = p.get("since");
  const until = p.get("until");
  const status = p.get("status");
  return runs.filter((r) => {
    if (project && r.project !== project) return false;
    if (suite && r.suite !== suite) return false;
    if (tag && !r.tags.includes(tag)) return false;
    if (branch && r.git_branch !== branch) return false;
    if (since && r.created_at < Number(since)) return false;
    if (until && r.created_at > Number(until)) return false;
    if (status && derivedRunStatus(r) !== status) return false;
    return true;
  });
}

function filterCases(cases: CaseRow[], p: URLSearchParams): CaseRow[] {
  const status = p.get("status");
  const tag = p.get("tag");
  const q = p.get("q")?.toLowerCase().trim();
  return cases.filter((c) => {
    if (status && c.status !== status) return false;
    if (tag && !c.tags.includes(tag)) return false;
    if (q) {
      const hay = `${c.name ?? ""} ${c.output_preview ?? ""} ${c.case_key}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

export async function mockFetch(rawUrl: string, init: RequestInit = {}): Promise<Response> {
  const url = new URL(rawUrl, "http://mock.local");
  const method = (init.method ?? "GET").toUpperCase();
  const p = url.searchParams;

  // Strip the API base and split into segments.
  const path = url.pathname.replace(/^\/api\/v1/, "");
  const seg = path.split("/").filter(Boolean).map(decodeURIComponent);

  // GET /meta
  if (method === "GET" && seg[0] === "meta" && seg.length === 1) {
    return json(fx.META);
  }

  // /runs...
  if (seg[0] === "runs") {
    // GET /runs
    if (method === "GET" && seg.length === 1) {
      const filtered = filterRuns(fx.allRunSummaries(), p);
      const { page, next_cursor } = paginate(filtered, p);
      return json({ runs: page, next_cursor });
    }
    const runId = seg[1];
    // GET /runs/:id
    if (method === "GET" && seg.length === 2) {
      try {
        const summary = fx.summarizeRun(runId);
        return json({ ...summary, assert_labels: fx.runAssertLabels(runId) });
      } catch {
        return notFound();
      }
    }
    if (seg[2] === "cases") {
      // GET /runs/:id/cases
      if (method === "GET" && seg.length === 3) {
        const filtered = filterCases(fx.runCases(runId), p);
        const { page, next_cursor } = paginate(filtered, p);
        // Return lean rows (drop nothing extra; CaseRow is already lean).
        return json({ cases: page, next_cursor });
      }
      // GET /runs/:id/cases/:case_key
      if (method === "GET" && seg.length === 4) {
        const detail = fx.caseDetail(runId, seg[3]);
        return detail ? json(detail) : notFound();
      }
    }
    if (seg[2] === "compare" && method === "GET") {
      const other = seg[3] ?? fx.defaultCompareTarget(runId);
      if (!other) return json({ error: "no_baseline" }, 404);
      const result = fx.compareRuns(other, runId);
      return result ? json(result) : notFound();
    }
  }

  // /projects...
  if (seg[0] === "projects") {
    if (method === "GET" && seg.length === 1) {
      return json({ projects: fx.projectSummaries() });
    }
    const project = seg[1];
    if (seg[2] === "suites") {
      // GET /projects/:project/suites
      if (method === "GET" && seg.length === 3) {
        return json({ project, suites: fx.suiteSummaries(project) });
      }
      // PUT /projects/:project/suites/:suite/baseline
      if (method === "PUT" && seg[4] === "baseline" && seg.length === 5) {
        const suite = seg[3];
        let runId = "";
        try {
          runId = JSON.parse(String(init.body ?? "{}")).run_id ?? "";
        } catch {
          /* ignore */
        }
        if (!runId) return json({ error: "run_id required" }, 400);
        fx.setSuiteBaseline(project, suite, runId);
        return json({ project, suite, run_id: runId });
      }
    }
  }

  // /cache...
  if (seg[0] === "cache") {
    if (method === "GET" && seg[1] === "stats") {
      return json(fx.cacheStats());
    }
    if (method === "POST" && seg[1] === "prune") {
      // Admin action: exercise the 401 -> token modal flow when unauthenticated.
      const authed = (init.headers as Record<string, string> | undefined)?.[
        "Authorization"
      ];
      if (!authed) return json({ error: "unauthorized" }, 401);
      return json({ pruned: 128, freed_bytes: 12_582_912 });
    }
  }

  return notFound();
}
