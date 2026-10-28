import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { AuthProvider, useAuth } from "@/auth/AuthProvider";
import { setMockEnabled } from "@/mocks/handlers";
import { SetupPage } from "./SetupPage";

// A stubbed `fetch`, not the shared mock: the mock's setup handler accepts
// any password, and the point here is what the page does with a refusal.

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

function stubServer(setupResponse: () => Response) {
  const fetchMock = vi.fn((input: string) => {
    const path = new URL(input, "http://localhost").pathname.replace(
      /^\/api\/v1/,
      "",
    );
    if (path === "/meta") {
      return Promise.resolve(json({ auth_mode: "closed", setup_required: true }));
    }
    if (path === "/auth/me") {
      return Promise.resolve(
        json({ authenticated: false, user: null, source: "anonymous", scope: null }),
      );
    }
    if (path === "/auth/setup") return Promise.resolve(setupResponse());
    return Promise.resolve(json({ error: "not found" }, 404));
  });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

/** The app's boot gate waits for auth before routing; mirror that here, or
 *  `SetupPage` redirects away before `setup_required` has loaded. */
function AfterAuth() {
  const { isLoading } = useAuth();
  return isLoading ? null : <SetupPage />;
}

function renderSetupPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <MemoryRouter initialEntries={["/setup"]}>
          <Routes>
            <Route path="/setup" element={<AfterAuth />} />
            <Route path="/" element={<p>home</p>} />
          </Routes>
        </MemoryRouter>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

async function fillForm(password: string) {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText("Username"), "founder");
  await user.type(screen.getByLabelText("Password"), password);
  await user.type(screen.getByLabelText("Confirm password"), password);
  await user.click(screen.getByRole("button", { name: /Create admin/ }));
}

beforeEach(() => setMockEnabled(false));
afterEach(() => {
  setMockEnabled(true);
  vi.unstubAllGlobals();
});

describe("SetupPage", () => {
  it("states the server's 8-character minimum up front", async () => {
    stubServer(() => json({}, 201));
    renderSetupPage();
    expect(
      await screen.findByText("At least 8 characters."),
    ).toBeInTheDocument();
  });

  // The server refuses anything under 8; a 4-character client check let a
  // 7-character password through to a generic "Setup failed".
  it("rejects a 7-character password without calling the server", async () => {
    const fetchMock = stubServer(() => json({}, 201));
    renderSetupPage();
    await fillForm("s3cret!");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Choose a password of at least 8 characters.",
    );
    expect(
      fetchMock.mock.calls.some(([input]) =>
        input.includes("/auth/setup"),
      ),
    ).toBe(false);
  });

  it("shows the server's sentence when setup is refused", async () => {
    stubServer(() => json({ error: "setup has already been completed" }, 409));
    renderSetupPage();
    await fillForm("s3cret-pass");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "setup has already been completed",
    );
    expect(screen.getByRole("button", { name: /Create admin/ })).toBeEnabled();
  });
});
