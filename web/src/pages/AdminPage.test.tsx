import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

async function createUser(username: string, password: string) {
  const user = userEvent.setup();
  await user.type(
    await screen.findByRole("textbox", { name: "Username" }),
    username,
  );
  await user.type(screen.getByLabelText("Password"), password);
  await user.click(screen.getByRole("button", { name: "Create user" }));
  return user;
}

// The page shows whatever sentence the server refused with. It used to match
// codes (`username_taken`, `last_admin`) only the mocks sent, so against the
// real server every refusal read as generic copy.
describe("AdminPage server refusals", () => {
  it("shows the server's sentence when a username is taken", async () => {
    renderAdminPage();
    await screen.findByTestId("user-row-member");
    await createUser("member", "long-enough");
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "username already exists",
    );
  });

  it("shows the server's sentence when deleting the last admin", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    const row = await screen.findByTestId("user-row-admin");
    await user.click(within(row).getByRole("button", { name: "Delete" }));
    await user.click(screen.getByRole("button", { name: "Delete user" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "cannot delete the last admin",
    );
  });

  it("shows a reset-password refusal inside the still-open dialog", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    const row = await screen.findByTestId("user-row-member");
    await user.click(within(row).getByRole("button", { name: "Reset password" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("New password"), "short");
    await user.click(within(dialog).getByRole("button", { name: "Set password" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "password must be at least 8 characters",
    );
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

describe("AdminPage success notices", () => {
  it("confirms a created user", async () => {
    renderAdminPage();
    await screen.findByTestId("user-row-member");
    await createUser("tester", "long-enough");
    expect(await screen.findByRole("status")).toHaveTextContent(
      'Created user "tester".',
    );
    expect(await screen.findByTestId("user-row-tester")).toBeInTheDocument();
  });

  it("confirms a password reset", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    const row = await screen.findByTestId("user-row-member");
    await user.click(within(row).getByRole("button", { name: "Reset password" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("New password"), "long-enough");
    await user.click(within(dialog).getByRole("button", { name: "Set password" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("status")).toHaveTextContent(
      'Password reset for "member".',
    );
  });
});

// The server only refuses the change that leaves no enabled admin. With a
// second admin around, the signed-in admin could demote or disable themselves
// in a single click and lose this page.
describe("AdminPage confirmations", () => {
  async function promoteMember(user: ReturnType<typeof userEvent.setup>) {
    // Make a second admin so the server-side last-admin guard is out of play.
    const select = await screen.findByLabelText("Role for member");
    await user.selectOptions(select, "admin");
    await waitFor(() => expect(select).toHaveValue("admin"));
  }

  it("asks before the signed-in admin demotes themselves", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    await promoteMember(user);
    const own = screen.getByLabelText("Role for admin");
    await user.selectOptions(own, "member");

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Change your own role");
    // Nothing has changed until the admin confirms.
    expect(own).toHaveValue("admin");

    await user.click(within(dialog).getByRole("button", { name: "Change to member" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() =>
      expect(screen.getByLabelText("Role for admin")).toHaveValue("member"),
    );
  });

  it("leaves the role alone when the self-demotion is cancelled", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    await promoteMember(user);
    await user.selectOptions(screen.getByLabelText("Role for admin"), "viewer");
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByLabelText("Role for admin")).toHaveValue("admin");
  });

  it("changes another user's role without asking", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    const select = await screen.findByLabelText("Role for member");
    await user.selectOptions(select, "viewer");
    await waitFor(() => expect(select).toHaveValue("viewer"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("asks before disabling any account", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    const row = await screen.findByTestId("user-row-member");
    await user.click(within(row).getByRole("button", { name: "Disable" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent('Disable "member"?');
    expect(within(row).getByText("active")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "Disable user" }));
    expect(await within(row).findByText("disabled")).toBeInTheDocument();
  });

  it("words the disable confirmation for the signed-in admin's own row", async () => {
    const user = userEvent.setup();
    renderAdminPage();
    const row = await screen.findByTestId("user-row-admin");
    await user.click(within(row).getByRole("button", { name: "Disable" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Disable your own account");
    expect(
      within(dialog).getByRole("button", { name: "Disable my account" }),
    ).toBeInTheDocument();
  });
});
