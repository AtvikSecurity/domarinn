import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import type { MeResponse } from "@/api";
import { AuthProvider } from "@/auth/AuthProvider";
import { setMockEnabled } from "@/mocks/handlers";
import { LoginPage } from "./LoginPage";

// Driven against a stubbed `fetch`: the shared mock backend never throttles.

const ANONYMOUS: MeResponse = {
  authenticated: false,
  user: null,
  source: "anonymous",
  scope: null,
};

function json(body: unknown, status = 200, headers?: HeadersInit): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });
}

function stubServer(login: () => Response) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string) => {
      const path = new URL(input, "http://localhost").pathname.replace(
        /^\/api\/v1/,
        "",
      );
      if (path === "/meta") {
        return Promise.resolve(
          json({ auth_mode: "closed", setup_required: false }),
        );
      }
      if (path === "/auth/me") return Promise.resolve(json(ANONYMOUS));
      if (path === "/auth/login") return Promise.resolve(login());
      return Promise.resolve(json({ error: "not found" }, 404));
    }),
  );
}

function renderLoginPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <MemoryRouter initialEntries={["/login"]}>
          <LoginPage />
        </MemoryRouter>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

async function signIn() {
  const user = userEvent.setup();
  renderLoginPage();
  await user.type(await screen.findByLabelText("Username"), "root");
  await user.type(screen.getByLabelText("Password"), "wrong-password");
  await user.click(screen.getByRole("button", { name: "Sign in" }));
}

beforeEach(() => setMockEnabled(false));
afterEach(() => {
  setMockEnabled(true);
  vi.unstubAllGlobals();
});

describe("LoginPage failures", () => {
  it("keeps a 401 generic", async () => {
    stubServer(() => json({ error: "invalid credentials" }, 401));
    await signIn();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Invalid username or password.",
    );
  });

  // The throttle's 429 used to read as "Sign in failed. Please try again.",
  // which invites exactly the retry that keeps the account locked.
  it("tells a throttled user how long to wait", async () => {
    stubServer(() =>
      json(
        { error: "too many failed password attempts; try again in 14 minutes" },
        429,
        { "Retry-After": "812" },
      ),
    );
    await signIn();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "too many failed password attempts; try again in 14 minutes",
    );
  });

  it("still says to wait when a 429 has no sentence", async () => {
    stubServer(() => new Response(null, { status: 429 }));
    await signIn();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      /Too many failed sign-in attempts/,
    );
  });
});
