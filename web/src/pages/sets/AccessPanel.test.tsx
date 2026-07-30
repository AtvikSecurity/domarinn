import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { AuthProvider } from "@/auth/AuthProvider";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { createUser, login, resetMockAuth } from "@/mocks/authState";
import * as fx from "@/mocks/fixtures";
import { AccessPanel } from "./AccessPanel";

beforeEach(() => {
  resetMockAuth();
  fx.resetSets();
});

/**
 * `support-bot` is restricted at the PROJECT level, so the project panel sees
 * covering and exact agree — which is exactly why the suite variant below
 * matters: `faq-accuracy` is covered by that lock and owns no row of its own.
 * `coveringRestricted` is what the browse pages pass down.
 */
function renderPanel(
  { suite, coveringRestricted }: { suite?: string | null; coveringRestricted?: boolean } = {},
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <TooltipProvider>
          <MemoryRouter>
            <AccessPanel
              project="support-bot"
              suite={suite ?? null}
              coveringRestricted={coveringRestricted ?? true}
              open
              onClose={() => {}}
            />
          </MemoryRouter>
        </TooltipProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

/** Sign in as the seeded non-admin who holds `manage` over `support-bot`. */
function signInAsManager() {
  login("member", "member");
}

/**
 * Sign in as a viewer-role account holding `manage`. The asymmetry this exists
 * to cover: the GET is read-scoped, so the panel LOADS, while every mutation
 * needs write scope and would 403.
 */
function signInAsReadOnlyManager() {
  // `protect-writes`, because in `open` mode the server lets anyone write and
  // the UI correctly follows it — the read-only case only exists where writes
  // are actually gated. The mock reads the mode off this flag.
  localStorage.setItem("domarinn.mock.authmode", "protect-writes");
  const user = createUser("watcher", "watcher", "viewer");
  fx.upsertGrant("support-bot", null, user!.id, user!.username, "manage", "admin");
  login("watcher", "watcher");
}

describe("AccessPanel", () => {
  it("lists who holds the set, and at what level", async () => {
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    expect(
      await within(dialog).findByRole("combobox", { name: "Level for member" }),
    ).toHaveValue("manage");
    expect(
      within(dialog).getByRole("combobox", { name: "Level for sso.only" }),
    ).toHaveValue("view");
  });

  it("writes a level change through to the set", async () => {
    const user = userEvent.setup();
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    await user.selectOptions(
      await within(dialog).findByRole("combobox", { name: "Level for member" }),
      "upload",
    );

    await waitFor(() => {
      const grants = fx.setAccess("support-bot", null).grants;
      expect(grants.find((g) => g.username === "member")?.level).toBe("upload");
    });
  });

  it("removes a grant", async () => {
    const user = userEvent.setup();
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    await user.click(
      await within(dialog).findByRole("button", { name: "Remove sso.only" }),
    );

    await waitFor(() => {
      expect(
        fx.setAccess("support-bot", null).grants.map((g) => g.username),
      ).toEqual(["member"]);
    });
  });

  it("keeps the restriction toggle to admins", async () => {
    // A manage grant administers its set's access list; locking and unlocking
    // the set stays with the operator, and the server 403s a manager who tries.
    signInAsManager();
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByRole("combobox", { name: "Level for member" });
    expect(
      within(dialog).queryByRole("button", { name: /Unlock|Restrict/ }),
    ).toBeNull();
  });

  it("does not offer a non-admin manager the add-grant row", async () => {
    // The user list is an admin-only endpoint and the grant PUT is keyed by
    // user id, so there is nothing a manager could pick from.
    signInAsManager();
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByRole("combobox", { name: "Level for member" });
    expect(within(dialog).queryByRole("combobox", { name: "Add person" })).toBeNull();
    expect(
      within(dialog).getByText(/Ask an admin to add new people/i),
    ).toBeInTheDocument();
  });

  it("offers an admin the restriction toggle behind a confirm step", async () => {
    const user = userEvent.setup();
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    await user.click(
      await within(dialog).findByRole("button", { name: "Unlock project" }),
    );
    // The confirm is a step inside this modal, not a second dialog stacked on it.
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    await user.click(within(dialog).getByRole("button", { name: "Unlock project" }));

    await waitFor(() => {
      expect(fx.setAccess("support-bot", null).restricted).toBe(false);
    });
  });

  it("is read-only for a manager whose credential cannot write", async () => {
    // The GET is read-scoped, so the panel loads; every mutation would 403,
    // so none of the affordances that lead to one are rendered.
    signInAsReadOnlyManager();
    renderPanel();
    const dialog = await screen.findByRole("dialog");
    expect(
      await within(dialog).findByText(/read-only credential/i),
    ).toBeInTheDocument();
    expect(within(dialog).queryByRole("combobox", { name: "Level for member" })).toBeNull();
    expect(within(dialog).queryByRole("button", { name: "Remove member" })).toBeNull();
    // The list itself is still readable — that is the point of loading it.
    expect(within(dialog).getByText("member")).toBeInTheDocument();
  });

  it("states the covering visibility, not the row this suite happens to own", async () => {
    // `faq-accuracy` sits inside a locked project and owns no restriction row.
    // Reading the panel's own exact-scope flag would print "anyone who can read
    // this server can see this set's runs" over a set nobody outside the
    // project's grants can see.
    renderPanel({ suite: "faq-accuracy" });
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("restricted");
    expect(
      within(dialog).getByText(/inherited from support-bot/i),
    ).toBeInTheDocument();
    expect(
      within(dialog).queryByText(/Anyone who can read this server/i),
    ).toBeNull();

    // The toggle still describes the row it would write: this suite has none,
    // so the verb is "Restrict", not "Unlock".
    expect(
      within(dialog).getByRole("button", { name: "Restrict suite" }),
    ).toBeInTheDocument();
  });

  it("says an unrestricted set is open", async () => {
    // The other side of the same wire: an open project must not inherit the
    // amber chip from a stale default.
    renderPanel({ coveringRestricted: false });
    const dialog = await screen.findByRole("dialog");
    expect(
      await within(dialog).findByText(/Anyone who can read this server/i),
    ).toBeInTheDocument();
    expect(within(dialog).queryByText(/inherited from/i)).toBeNull();
  });
});
