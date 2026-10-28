import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import type { ApiKeyView, AuthMode, MeResponse } from "@/api";
import { AuthProvider } from "@/auth/AuthProvider";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { setMockEnabled } from "@/mocks/handlers";
import { KeysPage } from "./KeysPage";

// These tests drive the page against a stubbed `fetch` rather than the shared
// mock backend: the mock seeds no viewer, its open-mode static principal has
// a user (the real server's never does), and it never refuses a revoke.

const VIEWER: MeResponse = {
  authenticated: true,
  user: {
    id: "u_viewer",
    username: "viewer",
    role: "viewer",
    identities: [],
    role_managed_by: null,
  },
  source: "session",
  scope: "read",
};

const STATIC_TOKEN: MeResponse = {
  authenticated: true,
  user: null,
  source: "static",
  scope: "admin",
};

const KEY: ApiKeyView = {
  id: "key_1",
  name: "CI key",
  prefix: "domarinn_abc",
  scope: "read",
  created_at: "2026-06-01T12:00:00Z",
  last_used_at: null,
  revoked: false,
};

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

interface Server {
  mode: AuthMode;
  me: MeResponse;
  /** Answer the key endpoints; `undefined` falls through to a 404. */
  apikeys?: (method: string, path: string) => Response | undefined;
}

function stubServer(server: Server) {
  const fetchMock = vi.fn((input: string, init?: RequestInit) => {
    const url = new URL(input, "http://localhost");
    const path = url.pathname.replace(/^\/api\/v1/, "");
    const method = (init?.method ?? "GET").toUpperCase();
    if (path === "/meta") {
      return Promise.resolve(
        json({ auth_mode: server.mode, setup_required: false }),
      );
    }
    if (path === "/auth/me") return Promise.resolve(json(server.me));
    if (path.startsWith("/apikeys")) {
      const res = server.apikeys?.(method, path);
      if (res) return Promise.resolve(res);
    }
    return Promise.resolve(json({ error: "not found" }, 404));
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function renderKeysPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/keys"]}>
            <KeysPage />
          </MemoryRouter>
        </TooltipProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => setMockEnabled(false));
afterEach(() => {
  setMockEnabled(true);
  vi.unstubAllGlobals();
});

describe("KeysPage access", () => {
  // The server gates the key endpoints at `read`: a viewer mints its own
  // read-only key. Gating the form on write scope locked viewers out.
  it("shows a viewer the create form, capped at read", async () => {
    stubServer({
      mode: "closed",
      me: VIEWER,
      apikeys: (method) =>
        method === "GET" ? json({ keys: [] }) : undefined,
    });
    renderKeysPage();
    const select = await screen.findByLabelText("Key scope");
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.getAttribute("value")),
    ).toEqual(["read"]);
    expect(screen.getByRole("button", { name: "Create key" })).toBeEnabled();
  });

  // A static token is authenticated but owned by nobody; the server refuses
  // it with `require_user`, so the page explains instead of offering a form
  // that can only fail.
  it("explains why a session with no user account cannot manage keys", async () => {
    const fetchMock = stubServer({ mode: "protect-writes", me: STATIC_TOKEN });
    renderKeysPage();
    expect(
      await screen.findByText(/static token is not tied to one/),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Key scope")).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Go to sign in" }),
    ).toBeInTheDocument();
    // And the key list is never requested on its behalf.
    expect(
      fetchMock.mock.calls.some(([input]) => input.includes("/apikeys")),
    ).toBe(false);
  });

  it("explains the same to an anonymous open-mode visitor", async () => {
    stubServer({
      mode: "open",
      me: { authenticated: false, user: null, source: "anonymous", scope: null },
    });
    renderKeysPage();
    expect(
      await screen.findByText(/API keys belong to a user account/),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Key scope")).not.toBeInTheDocument();
  });
});

describe("KeysPage failures", () => {
  it("renders a revoke refusal inside the still-open dialog", async () => {
    stubServer({
      mode: "closed",
      me: VIEWER,
      apikeys: (method) => {
        if (method === "GET") return json({ keys: [KEY] });
        if (method === "DELETE") return json({ error: "not your api key" }, 403);
        return undefined;
      },
    });
    const user = userEvent.setup();
    renderKeysPage();
    await user.click(await screen.findByRole("button", { name: "Revoke" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Revoke key" }));
    expect(
      await within(dialog).findByText("not your api key"),
    ).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    // Cancelling clears the error so the next attempt starts clean.
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await user.click(screen.getByRole("button", { name: "Revoke" }));
    expect(
      within(await screen.findByRole("dialog")).queryByText("not your api key"),
    ).not.toBeInTheDocument();
  });

  it("renders the server's sentence when creating a key fails", async () => {
    stubServer({
      mode: "closed",
      me: VIEWER,
      apikeys: (method) => {
        if (method === "GET") return json({ keys: [] });
        if (method === "POST") {
          return json({ error: "requested scope exceeds your own" }, 403);
        }
        return undefined;
      },
    });
    const user = userEvent.setup();
    renderKeysPage();
    await user.click(await screen.findByRole("button", { name: "Create key" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "requested scope exceeds your own",
    );
    expect(screen.queryByTestId("api-key-secret")).not.toBeInTheDocument();
  });
});
