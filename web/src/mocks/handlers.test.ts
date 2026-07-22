import { describe, expect, it } from "vitest";
import { mockFetch } from "./handlers";
import type {
  CaseHistoryResponse,
  CaseListResponse,
  CompareResponse,
  MatrixResponse,
  RunConfigResponse,
} from "@/api";

const MATRIX_RUN = "search-rerank-ndcg-eval-10";
const MONEY_RUN = "checkout-agent-regression-12";

// Pins the mock's `GET /runs/{id}/compare/{other}` response shape against the
// real server's contract: `Path((id, other))` -> `storage.compare_runs(id,
// other)` -> `{ base: id, head: other }` (see
// crates/domarinn-server/tests/compare.rs: `GET /runs/base/compare/head`
// asserts `body["base"] == "base"`). The FIRST url segment is always base,
// the SECOND is always head — the mock must not reverse them.
describe("mockFetch: GET /runs/:id/compare/:other", () => {
  it("returns base === the first url segment, head === the second", async () => {
    const res = await mockFetch(
      "/api/v1/runs/checkout-agent-regression-11/compare/checkout-agent-regression-12",
    );
    expect(res.status).toBe(200);
    const body = (await res.json()) as CompareResponse;
    expect(body.base).toBe("checkout-agent-regression-11");
    expect(body.head).toBe("checkout-agent-regression-12");
  });

  it("keeps returning the right base/head when the pair is reversed in the url", async () => {
    const res = await mockFetch(
      "/api/v1/runs/checkout-agent-regression-12/compare/checkout-agent-regression-11",
    );
    expect(res.status).toBe(200);
    const body = (await res.json()) as CompareResponse;
    expect(body.base).toBe("checkout-agent-regression-12");
    expect(body.head).toBe("checkout-agent-regression-11");
  });

  it("404s the two-segment form with no comparison target, like the real server (no such route)", async () => {
    const res = await mockFetch("/api/v1/runs/checkout-agent-regression-12/compare");
    expect(res.status).toBe(404);
  });
});

// Pins the mock's `GET /runs/{id}/matrix` against the generated `MatrixResponse`
// wire shape (crates/domarinn-server/src/dto/matrix.rs) and the server's pivot
// semantics (storage/matrix.rs): complete first-seen columns, paginated test
// rows, cells aligned 1:1 with the columns.
describe("mockFetch: GET /runs/:id/matrix", () => {
  it("returns the matrix wire shape for the matrix-shaped run", async () => {
    const res = await mockFetch(`/api/v1/runs/${MATRIX_RUN}/matrix`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as MatrixResponse;

    expect(body.run_id).toBe(MATRIX_RUN);
    expect(body.columns).toHaveLength(6); // 3 providers × 2 prompts
    expect(new Set(body.columns.map((c) => c.provider_id)).size).toBe(3);
    expect(body.rows).toHaveLength(12);
    for (const row of body.rows) {
      expect(row.cells).toHaveLength(body.columns.length);
      const first = row.cells[0]!;
      // Cell carries the flakiness signals the matrix view reads.
      expect(typeof first.pass_fraction).toBe("number");
      expect(typeof first.distinct_outputs).toBe("number");
      expect(Array.isArray(first.case_keys)).toBe(true);
    }
  });

  it("collapses a single-provider run to one column", async () => {
    const res = await mockFetch(`/api/v1/runs/${MONEY_RUN}/matrix`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as MatrixResponse;
    expect(body.columns).toEqual([{ provider_id: "openai", prompt_id: null }]);
  });

  it("paginates the test rows via ?limit/?cursor", async () => {
    const first = (await (
      await mockFetch(`/api/v1/runs/${MATRIX_RUN}/matrix?limit=5`)
    ).json()) as MatrixResponse;
    expect(first.rows).toHaveLength(5);
    // Columns are always complete, even on a partial row page.
    expect(first.columns).toHaveLength(6);
    expect(first.next_cursor).not.toBeNull();

    const second = (await (
      await mockFetch(
        `/api/v1/runs/${MATRIX_RUN}/matrix?limit=5&cursor=${first.next_cursor}`,
      )
    ).json()) as MatrixResponse;
    const firstTests = new Set(first.rows.map((r) => r.test_id));
    for (const row of second.rows) expect(firstTests.has(row.test_id)).toBe(false);
  });

  it("404s an unknown run", async () => {
    const res = await mockFetch("/api/v1/runs/no-such-run/matrix");
    expect(res.status).toBe(404);
  });
});

// `GET /runs/{id}/config` (Task 3 wire shape) + the deterministic config-drift
// fixture: the featured regression suite's final run (regression-12) bumps its
// config so the compare view's drift badge/panel have real data to render.
describe("mockFetch: GET /runs/:id/config", () => {
  it("returns the RunConfigResponse shape (run_id, digest, snapshot)", async () => {
    const res = await mockFetch(`/api/v1/runs/${MONEY_RUN}/config`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as RunConfigResponse;
    expect(body.run_id).toBe(MONEY_RUN);
    expect(typeof body.config_digest).toBe("string");
    expect(body.config_digest).toMatch(/^blake3:/);
    // The snapshot is a structured config document with a prompt block.
    const config = body.config as Record<string, unknown>;
    expect(config.model).toBeTruthy();
    expect(config.prompt).toBeTruthy();
  });

  it("drifts the digest between regression-11 and regression-12, matching the compare's config block", async () => {
    const c11 = (await (
      await mockFetch("/api/v1/runs/checkout-agent-regression-11/config")
    ).json()) as RunConfigResponse;
    const c12 = (await (
      await mockFetch("/api/v1/runs/checkout-agent-regression-12/config")
    ).json()) as RunConfigResponse;
    expect(c11.config_digest).not.toBe(c12.config_digest);

    const cmp = (await (
      await mockFetch(
        "/api/v1/runs/checkout-agent-regression-11/compare/checkout-agent-regression-12",
      )
    ).json()) as CompareResponse;
    expect(cmp.config.changed).toBe(true);
    expect(cmp.config.base_digest).toBe(c11.config_digest);
    expect(cmp.config.head_digest).toBe(c12.config_digest);
  });

  it("keeps the digest stable for a same-config pair (no drift within the series)", async () => {
    const c09 = (await (
      await mockFetch("/api/v1/runs/checkout-agent-regression-09/config")
    ).json()) as RunConfigResponse;
    const c10 = (await (
      await mockFetch("/api/v1/runs/checkout-agent-regression-10/config")
    ).json()) as RunConfigResponse;
    expect(c09.config_digest).toBe(c10.config_digest);

    const cmp = (await (
      await mockFetch(
        "/api/v1/runs/checkout-agent-regression-09/compare/checkout-agent-regression-10",
      )
    ).json()) as CompareResponse;
    expect(cmp.config.changed).toBe(false);
  });

  it("404s an unknown run", async () => {
    const res = await mockFetch("/api/v1/runs/no-such-run/config");
    expect(res.status).toBe(404);
  });
});

// `GET /projects/{project}/suites/{suite}/cases/{case_key}/history` (Task 5 wire
// shape) + the newest-first / `output_changed` semantics
// (crates/domarinn-server/src/storage/history.rs). The response must carry the
// baseline run id and a `points[]` whose `output_changed` compares each point to
// the next-older one.
describe("mockFetch: GET /projects/:project/suites/:suite/cases/:case_key/history", () => {
  it("returns the CaseHistoryResponse wire shape, newest-first", async () => {
    const res = await mockFetch(
      "/api/v1/projects/checkout-agent/suites/regression/cases/case-0024/history",
    );
    expect(res.status).toBe(200);
    const body = (await res.json()) as CaseHistoryResponse;

    expect(body.project).toBe("checkout-agent");
    expect(body.suite).toBe("regression");
    expect(body.case_key).toBe("case-0024");
    expect(body.baseline_run_id).toBe("checkout-agent-regression-11");

    expect(body.points.length).toBeGreaterThanOrEqual(2);
    // Newest-first: the latest run leads, created_at strictly descending.
    expect(body.points[0]!.run_id).toBe("checkout-agent-regression-12");
    for (let i = 1; i < body.points.length; i++) {
      expect(Date.parse(body.points[i - 1]!.created_at)).toBeGreaterThan(
        Date.parse(body.points[i]!.created_at),
      );
    }
    // Every optional key is present on the wire (explicit, never omitted).
    const p0 = body.points[0]!;
    for (const key of [
      "run_id",
      "created_at",
      "status",
      "score",
      "output_hash",
      "output_changed",
      "prompt_tokens",
      "completion_tokens",
      "cost_usd",
      "latency_ms",
      "git_commit",
      "config_digest",
    ] as const) {
      expect(Object.prototype.hasOwnProperty.call(p0, key)).toBe(true);
    }
  });

  it("computes output_changed vs the next-older point, null at the oldest", async () => {
    const body = (await (
      await mockFetch(
        "/api/v1/projects/checkout-agent/suites/regression/cases/case-0024/history",
      )
    ).json()) as CaseHistoryResponse;

    for (let i = 0; i < body.points.length; i++) {
      const cur = body.points[i]!;
      const older = body.points[i + 1];
      const expected =
        older && cur.output_hash != null && older.output_hash != null
          ? cur.output_hash !== older.output_hash
          : null;
      expect(cur.output_changed).toBe(expected);
    }
    expect(body.points.at(-1)!.output_changed).toBeNull();
  });

  it("honours ?limit, keeping the newest runs", async () => {
    const body = (await (
      await mockFetch(
        "/api/v1/projects/checkout-agent/suites/regression/cases/case-0024/history?limit=3",
      )
    ).json()) as CaseHistoryResponse;
    expect(body.points.map((p) => p.run_id)).toEqual([
      "checkout-agent-regression-12",
      "checkout-agent-regression-11",
      "checkout-agent-regression-10",
    ]);
  });

  it("404s a case that no run of the suite carries", async () => {
    const res = await mockFetch(
      "/api/v1/projects/checkout-agent/suites/regression/cases/case-9999/history",
    );
    expect(res.status).toBe(404);
  });

  it("404s an unknown suite", async () => {
    const res = await mockFetch(
      "/api/v1/projects/checkout-agent/suites/no-such-suite/cases/case-0024/history",
    );
    expect(res.status).toBe(404);
  });
});

// The migration-3 provider/prompt/test server filters on `GET /runs/{id}/cases`.
describe("mockFetch: GET /runs/:id/cases provider/prompt filters", () => {
  it("narrows the case list to one provider", async () => {
    const all = (await (
      await mockFetch(`/api/v1/runs/${MATRIX_RUN}/cases?limit=1000`)
    ).json()) as CaseListResponse;
    expect(all.cases).toHaveLength(144);

    const filtered = (await (
      await mockFetch(
        `/api/v1/runs/${MATRIX_RUN}/cases?limit=1000&provider=gpt-5-mini`,
      )
    ).json()) as CaseListResponse;
    expect(filtered.cases).toHaveLength(48); // 2 prompts × 12 tests × 2 repeats
    for (const c of filtered.cases) expect(c.provider_id).toBe("gpt-5-mini");
  });

  it("narrows the case list to one prompt", async () => {
    const filtered = (await (
      await mockFetch(`/api/v1/runs/${MATRIX_RUN}/cases?limit=1000&prompt=baseline`)
    ).json()) as CaseListResponse;
    expect(filtered.cases).toHaveLength(72); // 3 providers × 12 tests × 2 repeats
    for (const c of filtered.cases) expect(c.prompt_id).toBe("baseline");
  });

  it("intersects provider + prompt filters", async () => {
    const filtered = (await (
      await mockFetch(
        `/api/v1/runs/${MATRIX_RUN}/cases?limit=1000&provider=claude-sonnet&prompt=cot-v2`,
      )
    ).json()) as CaseListResponse;
    expect(filtered.cases).toHaveLength(24); // 12 tests × 2 repeats
    for (const c of filtered.cases) {
      expect(c.provider_id).toBe("claude-sonnet");
      expect(c.prompt_id).toBe("cot-v2");
    }
  });
});
