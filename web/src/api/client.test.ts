import { afterEach, describe, expect, it, vi } from "vitest";
import { apiRequest, ApiError } from "./client";
import type { CaseListResponse, RunListResponse } from "@/api";
import { clearToken, onUnauthorized, setToken } from "@/lib/auth";
import { log } from "@/lib/logger";

afterEach(() => {
  clearToken();
  vi.restoreAllMocks();
});

describe("apiRequest against the mock", () => {
  it("fetches runs", async () => {
    const res = await apiRequest<RunListResponse>("/runs", { params: { limit: 10 } });
    expect(res.runs.length).toBe(10);
    expect(res.next_cursor).toBeTruthy();
  });

  it("passes filters through to the mock", async () => {
    const res = await apiRequest<RunListResponse>("/runs", {
      params: { project: "checkout-agent" },
    });
    expect(res.runs.every((r) => r.project === "checkout-agent")).toBe(true);
  });

  it("returns lean case rows for a run, without per-case tags or full detail fields", async () => {
    const runs = await apiRequest<RunListResponse>("/runs", { params: { limit: 1 } });
    const first = runs.runs[0];
    if (!first) throw new Error("mock must return at least one run");
    const id = first.id;
    const res = await apiRequest<CaseListResponse>(
      `/runs/${encodeURIComponent(id)}/cases`,
      { params: { limit: 5 } },
    );
    expect(res.cases.length).toBeLessThanOrEqual(5);
    expect(res.cases[0]).toHaveProperty("asserts");
    // The lean list projection carries neither per-case tags nor the full
    // case-detail fields (see the generated CaseListItem type).
    expect(res.cases[0]).not.toHaveProperty("tags");
    expect(res.cases[0]).not.toHaveProperty("rendered_prompt");
  });

  it("throws ApiError(404) for unknown paths", async () => {
    await expect(apiRequest("/nope")).rejects.toMatchObject({
      name: "ApiError",
      status: 404,
    });
  });

  it("logs an error on a failed (!ok) request before throwing", async () => {
    const errSpy = vi.spyOn(log, "error").mockImplementation(() => {});
    await expect(apiRequest("/nope")).rejects.toBeInstanceOf(ApiError);
    expect(errSpy).toHaveBeenCalledWith(
      "api request failed",
      expect.objectContaining({ status: 404, method: "GET" }),
    );
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
