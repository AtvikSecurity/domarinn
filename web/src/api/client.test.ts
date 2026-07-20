import { afterEach, describe, expect, it, vi } from "vitest";
import { apiRequest, ApiError } from "./client";
import type { CasesResponse, RunsResponse } from "./types";
import { clearToken, onUnauthorized, setToken } from "@/lib/auth";

afterEach(() => clearToken());

describe("apiRequest against the mock", () => {
  it("fetches runs", async () => {
    const res = await apiRequest<RunsResponse>("/runs", { params: { limit: 10 } });
    expect(res.runs.length).toBe(10);
    expect(res.next_cursor).toBeTruthy();
  });

  it("passes filters through to the mock", async () => {
    const res = await apiRequest<RunsResponse>("/runs", {
      params: { project: "checkout-agent" },
    });
    expect(res.runs.every((r) => r.project === "checkout-agent")).toBe(true);
  });

  it("returns lean case rows for a run", async () => {
    const runs = await apiRequest<RunsResponse>("/runs", { params: { limit: 1 } });
    const id = runs.runs[0].id;
    const res = await apiRequest<CasesResponse>(
      `/runs/${encodeURIComponent(id)}/cases`,
      { params: { limit: 5 } },
    );
    expect(res.cases.length).toBeLessThanOrEqual(5);
    expect(res.cases[0]).toHaveProperty("asserts");
    expect(res.cases[0]).not.toHaveProperty("rendered_prompt");
  });

  it("throws ApiError(404) for unknown paths", async () => {
    await expect(apiRequest("/nope")).rejects.toMatchObject({
      name: "ApiError",
      status: 404,
    });
  });

  it("gates the admin prune on a token and emits the unauthorized signal", async () => {
    const spy = vi.fn();
    const off = onUnauthorized(spy);

    await expect(apiRequest("/cache/prune", { method: "POST" })).rejects.toBeInstanceOf(
      ApiError,
    );
    expect(spy).toHaveBeenCalledOnce();

    setToken("admin-token");
    const ok = await apiRequest<{ pruned: number }>("/cache/prune", { method: "POST" });
    expect(ok.pruned).toBeGreaterThan(0);
    off();
  });
});
