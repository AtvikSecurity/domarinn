import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { useApiKeys, useUsers } from "./queries";
import { resetMockAuth } from "@/mocks/authState";

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

beforeEach(() => resetMockAuth());

// Regression guard for the user-reported production crash: `/admin` and
// `/keys` crashed with `s.data.map is not a function` against the real
// server. Cause: `GET /apikeys` returns `{"keys": [...]}` and `GET /users`
// returns `{"users": [...]}` (see generated ApiKeyListResponse.ts /
// UserListResponse.ts), but `useApiKeys`/`useUsers` declared bare arrays and
// the old mock wrongly returned bare arrays too, so tests never caught it.
// The mock now returns the real wrapped envelope; these hooks must unwrap it
// so the pages (which call `.map`/`.length` on the result) keep working.
describe("useApiKeys / useUsers unwrap the server's wrapped list envelope", () => {
  it("useApiKeys resolves to a plain array, not the {keys: [...]} envelope", async () => {
    const { result } = renderHook(() => useApiKeys(), { wrapper });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(Array.isArray(result.current.data)).toBe(true);
    // The exact failure mode reported in production: calling .map() on the
    // resolved data threw "s.data.map is not a function" when it was still
    // the wrapped {keys: [...]} object.
    expect(() => result.current.data!.map((k) => k.id)).not.toThrow();
  });

  it("useUsers resolves to a plain array, not the {users: [...]} envelope", async () => {
    const { result } = renderHook(() => useUsers(), { wrapper });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(Array.isArray(result.current.data)).toBe(true);
    expect(result.current.data!.length).toBeGreaterThan(0);
    expect(() => result.current.data!.map((u) => u.username)).not.toThrow();
    expect(result.current.data!.map((u) => u.username).sort()).toEqual([
      "admin",
      "member",
    ]);
  });
});
