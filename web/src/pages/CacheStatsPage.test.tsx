import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { AuthProvider } from "@/auth/AuthProvider";
import { clearToken, setToken } from "@/lib/auth";
import { login, resetMockAuth } from "@/mocks/authState";
import * as fx from "@/mocks/fixtures";
import { parseTimestamp } from "@/lib/format";
import { CacheStatsPage } from "./CacheStatsPage";

beforeEach(() => resetMockAuth());
afterEach(() => clearToken());

/** Sign in, carrying the session as a bearer token: the mock prune refuses a
 *  request without an Authorization header. */
function signIn(username: string) {
  const session = login(username, username);
  if (!session) throw new Error(`no seeded account ${username}`);
  setToken(session.token);
}

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <MemoryRouter>
          <CacheStatsPage />
        </MemoryRouter>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

describe("CacheStatsPage prune", () => {
  it("is not offered to a non-admin, since the endpoint is admin-only", async () => {
    signIn("member");
    renderPage();
    await screen.findByRole("heading", { name: "Prune cache" });
    expect(screen.queryByRole("button", { name: /Prune cache/ })).toBeNull();
    expect(screen.getByText(/Pruning by hand is an admin action/)).toBeInTheDocument();
  });

  it("reports how many entries a bare prune removed", async () => {
    const user = userEvent.setup();
    signIn("admin");
    renderPage();
    await user.click(await screen.findByRole("button", { name: "Prune cache…" }));
    expect(screen.getByText(/configured retention limits will be removed/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Confirm prune" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Pruned 128 entries.");
  });

  it("sends older_than_days and reports the server's count", async () => {
    const user = userEvent.setup();
    signIn("admin");
    renderPage();
    await user.type(await screen.findByLabelText("Older than (days)"), "30");
    await user.click(screen.getByRole("button", { name: "Prune cache…" }));
    expect(screen.getByText(/older than 30 days will be removed/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Confirm prune" }));

    // The mock counts against its own newest entry; a bare prune says 128.
    const created = fx.cacheEntryList().map((e) => parseTimestamp(e.created_at));
    const cutoff = Math.max(...created) - 30 * 86_400_000;
    const expected = created.filter((t) => t < cutoff).length;
    expect(expected).not.toBe(128);
    expect(await screen.findByRole("status")).toHaveTextContent(
      `Pruned ${expected} entries.`,
    );
  });

  it("refuses a fractional age before it reaches the server", async () => {
    const user = userEvent.setup();
    signIn("admin");
    renderPage();
    await user.type(await screen.findByLabelText("Older than (days)"), "1.5");
    expect(screen.getByRole("button", { name: "Prune cache…" })).toBeDisabled();
    expect(screen.getByText(/whole number of days/)).toBeInTheDocument();
  });
});
