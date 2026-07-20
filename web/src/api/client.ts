import { emitUnauthorized, getToken } from "@/lib/auth";
import { mockFetch, isMockEnabled } from "@/mocks/handlers";

export const API_BASE = "/api/v1";

export class ApiError extends Error {
  status: number;
  body?: unknown;
  constructor(status: number, message: string, body?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.body = body;
  }
}

export interface RequestOptions {
  method?: string;
  /** Query params; undefined/null/"" values are dropped. */
  params?: Record<string, string | number | undefined | null>;
  body?: unknown;
  signal?: AbortSignal;
  /**
   * When true a 401 still throws an ApiError but does NOT emit the app-wide
   * "prompt for token" signal. Used by the auth endpoints (login/setup/me),
   * whose 401s are handled inline by the login flow rather than the token modal.
   */
  skipAuthRedirect?: boolean;
}

function buildUrl(path: string, params?: RequestOptions["params"]): string {
  const url = new URL(
    API_BASE + path,
    typeof window !== "undefined" ? window.location.origin : "http://localhost",
  );
  if (params) {
    for (const [key, value] of Object.entries(params)) {
      if (value === undefined || value === null || value === "") continue;
      url.searchParams.set(key, String(value));
    }
  }
  return url.pathname + url.search;
}

/**
 * Central fetch wrapper: injects the bearer token, routes to the mock when
 * enabled, and turns 401s into an app-wide "prompt for token" signal.
 */
export async function apiRequest<T>(
  path: string,
  opts: RequestOptions = {},
): Promise<T> {
  const url = buildUrl(path, opts.params);
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) headers["Authorization"] = `Bearer ${token}`;

  const init: RequestInit = {
    method: opts.method ?? "GET",
    headers,
    signal: opts.signal,
  };
  if (opts.body !== undefined) {
    headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(opts.body);
  }

  const res = isMockEnabled()
    ? await mockFetch(url, init)
    : await fetch(url, init);

  if (res.status === 401) {
    if (!opts.skipAuthRedirect) emitUnauthorized();
    throw new ApiError(401, "Unauthorized", await safeBody(res));
  }
  if (!res.ok) {
    const body = await safeBody(res);
    throw new ApiError(
      res.status,
      `${res.status} ${res.statusText || "Request failed"}`,
      body,
    );
  }
  if (res.status === 204) return undefined as T;
  // No runtime validation of the parsed body against `T` here: `T` is always
  // one of the generated types in `@/api/generated/`, produced directly from
  // the server's own serializing structs (ts-rs, CI drift-checked — see
  // `crates/measurellm-server/src/export_api_types`), and the UI ships in the
  // same binary as the server that serves it. The residual risk is a
  // deployed-server/cached-UI version skew (e.g. a stale service worker or
  // browser tab outliving a redeploy); that's accepted rather than paying for
  // a schema-validation pass on every response.
  return (await res.json()) as T;
}

async function safeBody(res: Response): Promise<unknown> {
  try {
    const text = await res.text();
    if (!text) return undefined;
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  } catch {
    return undefined;
  }
}

export { isMockEnabled };
