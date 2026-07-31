import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { AuthProvider } from "@/auth/AuthProvider";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { resetMockAuth } from "@/mocks/authState";
import { AdminPage } from "./AdminPage";

beforeEach(() => resetMockAuth());

function renderAdminPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    // `TooltipProvider` mirrors the app root: the SSO provider badges on each
    // row use real tooltips, and Radix throws without a provider ancestor.
    <QueryClientProvider client={client}>
      <AuthProvider>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/admin"]}>
            <AdminPage />
          </MemoryRouter>
        </TooltipProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

// Both role pickers are hand-written `<option>` lists, so a role that exists
// on the server but not here is simply unassignable from the UI — the failure
// is silent and looks like the role does not exist.
describe("AdminPage role pickers", () => {
  it("offers every role when creating a user", async () => {
    renderAdminPage();
    const select = await screen.findByLabelText("New user role");
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.getAttribute("value")),
    ).toEqual(["viewer", "member", "admin"]);
  });

  it("offers every role when changing an existing user's role", async () => {
    renderAdminPage();
    const select = await screen.findByLabelText("Role for member");
    expect(
      within(select)
        .getAllByRole("option")
        .map((o) => o.getAttribute("value")),
    ).toEqual(["viewer", "member", "admin"]);
  });
});
