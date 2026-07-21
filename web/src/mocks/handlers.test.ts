import { describe, expect, it } from "vitest";
import { mockFetch } from "./handlers";
import type { CompareResponse } from "@/api";

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
