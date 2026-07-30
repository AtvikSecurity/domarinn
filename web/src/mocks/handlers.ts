// Fetch-level mock. When enabled, apiRequest routes here instead of the
// network, so the whole app (and tests) runs against the fixture dataset with
// no service worker and no backend.
//
// Every response constructed here matches the generated response type's wire
// shape exactly — wrapped list envelopes (`{keys: [...]}`, `{users: [...]}`),
// RFC3339 timestamps, etc. — so the mock exercises the same drift-prone paths
// the real server does.

import type {
  ApiKeyListResponse,
  AuthScope,
  CacheEntryListItem,
  CacheEntryListResponse,
  CaseListResponse,
  OkResponse,
  PruneResponse,
  Role,
  RunListItem,
  RunListResponse,
  UserListResponse,
} from "@/api";
import { scopeAtLeast } from "@/lib/authz";
import { parseTimestamp } from "@/lib/format";
import * as fx from "./fixtures";
import type { MockCaseRow } from "./fixtures";
import * as auth from "./authState";

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

function noContent(): Response {
  return new Response(null, { status: 204 });
}

function forbidden(): Response {
  return json({ error: "forbidden" }, 403);
}

/** Extract the bearer token from a request's headers, if present. */
function bearer(init: RequestInit): string | null {
  const headers = init.headers as Record<string, string> | undefined;
  const value = headers?.["Authorization"] ?? headers?.["authorization"];
  if (!value) return null;
  const match = /^Bearer\s+(.+)$/i.exec(value);
  return match?.[1] ?? null;
}

function readJson<T = Record<string, unknown>>(init: RequestInit): T {
  // The client always serializes bodies to a JSON string; ignore any other
  // BodyInit rather than stringifying it into "[object Object]".
  if (typeof init.body !== "string") return {} as T;
  try {
    return JSON.parse(init.body) as T;
  } catch {
    return {} as T;
  }
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

function derivedRunStatus(r: RunListItem): string {
  if (r.error_count > 0) return "error";
  if (r.fail_count > 0) return "fail";
  return "pass";
}

/**
 * Mirrors `storage/cachebrowse.rs`. The sort matters as much as the filters:
 * this is the first mock handler that orders anything, and the entries page
 * sorts SERVER-side, so a mock that returned insertion order would let a green
 * e2e hide a broken ordering.
 *
 * SQLite orders NULLs first ascending / last descending; `nullsLast` below
 * reproduces that rather than letting a JS comparator put them wherever.
 */
function filterCacheEntries(
  rows: CacheEntryListItem[],
  p: URLSearchParams,
): CacheEntryListItem[] {
  const kind = p.get("kind");
  const model = p.get("model");
  const q = p.get("q")?.toLowerCase();
  const since = p.get("since");
  const until = p.get("until");

  const matched = rows.filter((r) => {
    // The two pseudo-values name a state rather than a kind.
    if (kind === "unindexed") return !r.indexed;
    if (kind === "unparseable") return r.parseable === false;
    if (kind && r.kind !== kind) return false;
    if (model && r.model !== model) return false;
    if (since && parseTimestamp(r.created_at) < Number(since)) return false;
    if (until && parseTimestamp(r.created_at) > Number(until)) return false;
    if (q) {
      const haystack = `${r.output_preview ?? ""} ${r.request_summary ?? ""}`;
      if (!haystack.toLowerCase().includes(q)) return false;
    }
    return true;
  });

  const column = p.get("sort") ?? "created";
  const desc = (p.get("order") ?? "desc") === "desc";
  const value = (r: CacheEntryListItem): number | null => {
    switch (column) {
      case "size":
        return r.size;
      case "cost":
        return r.cost_usd;
      case "last_access":
        return r.last_access_at ? parseTimestamp(r.last_access_at) : null;
      default:
        return parseTimestamp(r.created_at);
    }
  };
  // The server excludes unknown cost outright when sorting by it, because
  // ordering by an unknown value is meaningless and the NULL tail also stops
  // its keyset pagination.
  const sortable =
    column === "cost" ? matched.filter((r) => r.cost_usd !== null) : matched;

  return [...sortable].sort((a, b) => {
    const av = value(a);
    const bv = value(b);
    if (av === null && bv === null) return a.key < b.key ? 1 : -1;
    if (av === null) return desc ? 1 : -1;
    if (bv === null) return desc ? -1 : 1;
    if (av === bv) return (a.key < b.key ? 1 : -1) * (desc ? 1 : -1);
    return desc ? bv - av : av - bv;
  });
}

/** Mirrors the server's FULLY_CACHED predicate (storage/runs.rs). */
function fullyCached(r: RunListItem): boolean {
  return r.cache_misses === 0 && (r.cache_hits ?? 0) > 0;
}

/** What `cached=exclude` hides: fully cached AND passing (verdicts are never
 *  cached, so a fully-cached failing run always stays visible). */
function hiddenByCachedExclude(r: RunListItem): boolean {
  return fullyCached(r) && r.fail_count === 0 && r.error_count === 0;
}

function filterRuns(runs: RunListItem[], p: URLSearchParams): RunListItem[] {
  const project = p.get("project");
  const suite = p.get("suite");
  const tag = p.get("tag");
  const branch = p.get("branch");
  const since = p.get("since");
  const until = p.get("until");
  const status = p.get("status");
  const cached = p.get("cached");
  const origin = p.get("origin");
  const actor = p.get("actor");
  return runs.filter((r) => {
    if (project && r.project !== project) return false;
    if (suite && r.suite !== suite) return false;
    if (tag && !r.tags.includes(tag)) return false;
    if (branch && r.git_branch !== branch) return false;
    if (since && parseTimestamp(r.created_at) < Number(since)) return false;
    if (until && parseTimestamp(r.created_at) > Number(until)) return false;
    if (status && derivedRunStatus(r) !== status) return false;
    if (cached === "exclude" && hiddenByCachedExclude(r)) return false;
    if (cached === "only" && !fullyCached(r)) return false;
    // Mirrors the server: `ci_provider IS NOT NULL` is the exact CI predicate,
    // and `actor` matches either who ran a run or who uploaded it.
    if (origin === "ci" && r.ci_provider == null) return false;
    if (origin === "local" && r.ci_provider != null) return false;
    if (actor && r.actor !== actor && r.uploaded_by !== actor) return false;
    return true;
  });
}

function filterCases(cases: MockCaseRow[], p: URLSearchParams): MockCaseRow[] {
  const status = p.get("status");
  const tag = p.get("tag");
  const q = p.get("q")?.toLowerCase().trim();
  // Matrix-identity server filters (exact match on the promoted columns), the
  // same ones the real `GET /runs/{id}/cases` accepts (crates/.../cases.rs).
  const provider = p.get("provider");
  const prompt = p.get("prompt");
  const test = p.get("test");
  const cached = p.get("cached");
  const errorClass = p.get("error_class");
  return cases.filter((c) => {
    if (status && c.status !== status) return false;
    if (errorClass && c.error_class !== errorClass) return false;
    if (tag && !c.tags.includes(tag)) return false;
    if (provider && c.provider_id !== provider) return false;
    if (prompt && c.prompt_id !== prompt) return false;
    if (test && c.test_id !== test) return false;
    if (cached === "true" && !c.cached) return false;
    if (cached === "false" && c.cached) return false;
    if (q) {
      const hay = `${c.name} ${c.output_preview} ${c.case_key}`.toLowerCase();
      if (!hay.includes(q)) return false;
    }
    return true;
  });
}

// Mirrors `fetch`'s async signature so the client can `await` the mock and the
// real `fetch` through one code path (awaiting a sync value would trip
// await-thenable at the call site). No handler awaits today, hence the one
// targeted require-await exemption.
// eslint-disable-next-line @typescript-eslint/require-await
export async function mockFetch(rawUrl: string, init: RequestInit = {}): Promise<Response> {
  const url = new URL(rawUrl, "http://mock.local");
  const method = (init.method ?? "GET").toUpperCase();
  const p = url.searchParams;

  // Strip the API base and split into segments.
  const path = url.pathname.replace(/^\/api\/v1/, "");
  const seg = path.split("/").filter(Boolean).map(decodeURIComponent);

  // GET /meta
  if (method === "GET" && seg[0] === "meta" && seg.length === 1) {
    return json({
      ...fx.META,
      auth_mode: auth.mockAuthMode(),
      setup_required: auth.setupRequired(),
    });
  }

  // Closed mode: every path except the always-open bootstrap surface
  // (health, meta, /auth/*) requires an authenticated caller. This mirrors
  // the real server's Scoped extractor so the closed-mode e2e / an expired
  // session actually exercises the 401 -> onUnauthorized -> /login path,
  // instead of the mock silently serving data. Open/protect-writes are
  // unchanged (existing specs browse anonymously as the static admin).
  if (
    auth.mockAuthMode() === "closed" &&
    seg[0] !== "health" &&
    seg[0] !== "auth" &&
    !auth.resolveAuth(bearer(init)).me.authenticated
  ) {
    return json({ error: "authentication required" }, 401);
  }

  // /auth/... — login, setup, logout, me
  if (seg[0] === "auth") {
    const token = bearer(init);
    if (method === "GET" && seg[1] === "me" && seg.length === 2) {
      return json(auth.resolveAuth(token).me);
    }
    if (method === "POST" && seg[1] === "login" && seg.length === 2) {
      const body = readJson<{ username?: string; password?: string }>(init);
      const res = auth.login(body.username ?? "", body.password ?? "");
      return res ? json(res) : json({ error: "invalid_credentials" }, 401);
    }
    if (method === "POST" && seg[1] === "setup" && seg.length === 2) {
      if (!auth.setupRequired()) return json({ error: "already_setup" }, 409);
      const body = readJson<{ username?: string; password?: string }>(init);
      return json(auth.setup(body.username ?? "", body.password ?? ""), 201);
    }
    if (method === "POST" && seg[1] === "logout" && seg.length === 2) {
      // Real server: 401 for an anonymous caller, otherwise `OkResponse`.
      const { me } = auth.resolveAuth(token);
      if (!me.authenticated) return json({ error: "authentication required" }, 401);
      auth.logout(token);
      const ok: OkResponse = { ok: true };
      return json(ok);
    }
  }

  // /apikeys — write scope
  if (seg[0] === "apikeys") {
    const { me, userId } = auth.resolveAuth(bearer(init));
    if (!scopeAtLeast(me.scope ?? undefined, "write")) return forbidden();
    if (method === "GET" && seg.length === 1) {
      const res: ApiKeyListResponse = { keys: auth.listApiKeys(userId) };
      return json(res);
    }
    if (method === "POST" && seg.length === 1) {
      const body = readJson<{ name?: string; scope?: AuthScope }>(init);
      return json(
        auth.createApiKey(userId, body.name, body.scope, me.scope ?? "read"),
        201,
      );
    }
    if (method === "DELETE" && seg.length === 2) {
      const keyId = seg[1];
      if (keyId === undefined) return notFound();
      return auth.revokeApiKey(userId, keyId) ? noContent() : notFound();
    }
  }

  // /users — admin scope
  if (seg[0] === "users") {
    const { me } = auth.resolveAuth(bearer(init));
    if (!scopeAtLeast(me.scope ?? undefined, "admin")) return forbidden();
    if (method === "GET" && seg.length === 1) {
      const res: UserListResponse = { users: auth.listUsers() };
      return json(res);
    }
    if (method === "POST" && seg.length === 1) {
      const body = readJson<{
        username?: string;
        password?: string;
        role?: Role;
      }>(init);
      const created = auth.createUser(body.username, body.password, body.role);
      return created ? json(created, 201) : json({ error: "username_taken" }, 409);
    }
    if (method === "PATCH" && seg.length === 2) {
      const targetId = seg[1];
      if (targetId === undefined) return notFound();
      const body = readJson<auth.UserPatch>(init);
      const res = auth.updateUser(targetId, body);
      if (res === "last_admin") return json({ error: "last_admin" }, 409);
      if (res === "not_found") return notFound();
      return json(res);
    }
    if (method === "DELETE" && seg.length === 2) {
      const targetId = seg[1];
      if (targetId === undefined) return notFound();
      const res = auth.deleteUser(targetId);
      if (res === "last_admin") return json({ error: "last_admin" }, 409);
      if (res === "not_found") return notFound();
      return noContent();
    }
  }

  // GET /search
  if (method === "GET" && seg[0] === "search" && seg.length === 1) {
    const limit = Math.min(Math.max(Number(p.get("limit") ?? 20) || 20, 1), 50);
    return json(fx.searchFixtures(p.get("q") ?? "", limit));
  }

  // /runs...
  if (seg[0] === "runs") {
    // GET /runs
    if (method === "GET" && seg.length === 1) {
      const all = fx.allRunSummaries();
      const filtered = filterRuns(all, p);
      const { page, next_cursor } = paginate(filtered, p);
      // Mirrors the server: the suppressed-run count is computed only on the
      // first page of a `cached=exclude` query, over the whole filtered set.
      let cached_hidden: number | null = null;
      if (p.get("cached") === "exclude" && !p.get("cursor")) {
        const params = new URLSearchParams(p);
        params.delete("cached");
        cached_hidden = filterRuns(all, params).filter(hiddenByCachedExclude).length;
      }
      const res: RunListResponse = { runs: page, next_cursor: next_cursor ?? null, cached_hidden };
      return json(res);
    }
    const runId = seg[1];
    if (runId === undefined) return notFound();
    // GET /runs/:id
    if (method === "GET" && seg.length === 2) {
      try {
        return json(fx.runDetail(runId));
      } catch {
        return notFound();
      }
    }
    // DELETE /runs/:id — 204, matching the server. The fixtures are generated
    // and immutable, so this does not actually remove the run; the UI's job is
    // to navigate away on success, and that is what the e2e asserts.
    if (method === "DELETE" && seg.length === 2) {
      return new Response(null, { status: 204 });
    }
    if (seg[2] === "cases") {
      // GET /runs/:id/cases
      if (method === "GET" && seg.length === 3) {
        const filtered = filterCases(fx.runCases(runId), p);
        const { page, next_cursor } = paginate(filtered, p);
        const res: CaseListResponse = {
          cases: page.map(fx.toCaseListItem),
          next_cursor: next_cursor ?? null,
        };
        return json(res);
      }
      // GET /runs/:id/cases/:case_key
      if (method === "GET" && seg.length === 4) {
        const caseKey = seg[3];
        if (caseKey === undefined) return notFound();
        const detail = fx.caseDetail(runId, caseKey);
        return detail ? json(detail) : notFound();
      }
    }
    // GET /runs/:id/matrix — provider × prompt × test pivot. Columns are always
    // complete; the test rows paginate via ?limit/?cursor. An unknown run is a
    // 404 (buildMatrix returns undefined), mirroring the server.
    if (seg[2] === "matrix" && method === "GET" && seg.length === 3) {
      const limitRaw = p.get("limit");
      const limit =
        limitRaw !== null && limitRaw.trim() !== "" && Number.isFinite(Number(limitRaw))
          ? Number(limitRaw)
          : undefined;
      const cursorRaw = p.get("cursor");
      const cursor =
        cursorRaw !== null && cursorRaw.trim() !== "" && Number.isFinite(Number(cursorRaw))
          ? Number(cursorRaw)
          : undefined;
      const matrix = fx.buildMatrix(runId, { limit, cursor });
      return matrix ? json(matrix) : notFound();
    }
    // GET /runs/:id/config — the run's config digest + snapshot (cheap
    // config-drift fetch; the real server extracts it from the stored blob).
    if (seg[2] === "config" && method === "GET" && seg.length === 3) {
      const cfg = fx.runConfig(runId);
      return cfg ? json(cfg) : notFound();
    }
    if (seg[2] === "compare" && method === "GET") {
      // Real server: `GET /runs/{id}/compare/{other}` only —
      // `Path((id, other))` requires both segments, so there is no route for
      // the target-less `/runs/{id}/compare`. Mirror that 404 rather than
      // synthesizing a default target here.
      if (seg.length !== 4) return notFound();
      const other = seg[3];
      if (other === undefined) return notFound();
      // First segment = base, second = head — matches
      // `storage.compare_runs(id, other)` -> `{ base: id, head: other }`
      // (crates/domarinn-server/tests/compare.rs pins this order).
      const result = fx.compareRuns(runId, other);
      return result ? json(result) : notFound();
    }
  }

  // /projects...
  if (seg[0] === "projects") {
    if (method === "GET" && seg.length === 1) {
      return json({ projects: fx.projectSummaries() });
    }
    const project = seg[1];
    if (project === undefined) return notFound();
    if (seg[2] === "suites") {
      // GET /projects/:project/suites
      if (method === "GET" && seg.length === 3) {
        return json({ project, suites: fx.suiteSummaries(project) });
      }
      // PUT /projects/:project/suites/:suite/baseline
      if (method === "PUT" && seg[4] === "baseline" && seg.length === 5) {
        const suite = seg[3];
        if (suite === undefined) return notFound();
        const runId = readJson<{ run_id?: string }>(init).run_id ?? "";
        if (!runId) return json({ error: "run_id required" }, 400);
        fx.setSuiteBaseline(project, suite, runId);
        return json({ project, suite, run_id: runId });
      }
      // GET /projects/:project/suites/:suite/cases/:case_key/history — one
      // case's timeline across the suite's recent runs. Points are newest-first;
      // an unknown project/suite/case (no run carries the case) is a 404, like
      // the real handler.
      if (
        method === "GET" &&
        seg[4] === "cases" &&
        seg[6] === "history" &&
        seg.length === 7
      ) {
        const suite = seg[3];
        const caseKey = seg[5];
        if (suite === undefined || caseKey === undefined) return notFound();
        const limitRaw = p.get("limit");
        const limit =
          limitRaw !== null &&
          limitRaw.trim() !== "" &&
          Number.isFinite(Number(limitRaw))
            ? Number(limitRaw)
            : undefined;
        const history = fx.caseHistory(project, suite, caseKey, limit);
        return history ? json(history) : notFound();
      }
    }
  }

  // /cache...
  if (seg[0] === "cache") {
    if (method === "GET" && seg[1] === "stats") {
      return json(fx.cacheStats());
    }
    if (method === "GET" && seg[1] === "facets") {
      return json(fx.cacheFacets());
    }
    if (method === "GET" && seg[1] === "entries" && seg.length === 2) {
      // The local tier is a different, smaller corpus — a developer's own
      // machine, not the shared server — so the switcher visibly changes what
      // is listed rather than re-rendering the same rows.
      const corpus =
        p.get("tier") === "local"
          ? fx.cacheEntryList().filter((_, i) => i % 4 === 0)
          : fx.cacheEntryList();
      const filtered = filterCacheEntries(corpus, p);
      const { page, next_cursor } = paginate(filtered, p);
      const res: CacheEntryListResponse = {
        entries: page,
        next_cursor: next_cursor ?? null,
        truncated: false,
      };
      return json(res);
    }
    if (method === "GET" && seg[1] === "entries" && seg.length === 3) {
      const key = decodeURIComponent(seg[2] as string);
      const detail = fx.cacheEntryDetail(key, p.get("raw") === "true");
      return detail ? json(detail) : notFound();
    }
    if (method === "POST" && seg[1] === "prune") {
      // Admin action: exercise the 401 -> /login redirect when unauthenticated.
      const authed = (init.headers as Record<string, string> | undefined)?.[
        "Authorization"
      ];
      if (!authed) return json({ error: "unauthorized" }, 401);
      const res: PruneResponse = { pruned: 128 };
      return json(res);
    }
  }

  return notFound();
}
