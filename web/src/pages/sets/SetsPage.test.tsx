import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { login, resetMockAuth } from "@/mocks/authState";
import * as fx from "@/mocks/fixtures";
import { SetsPage } from "./SetsPage";

beforeEach(() => {
  resetMockAuth();
  fx.resetSets();
});

function renderSets() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/sets"]}>
          <SetsPage />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("SetsPage", () => {
  it("says it is loading rather than rendering an empty table", async () => {
    renderSets();
    expect(screen.getByRole("status", { name: "Loading" })).toBeInTheDocument();
    await screen.findByRole("link", { name: "checkout-agent" });
  });

  it("links a row per project the caller may see", async () => {
    renderSets();
    expect(
      await screen.findByRole("link", { name: "checkout-agent" }),
    ).toHaveAttribute("href", "/sets/checkout-agent");
    // Name order, and the locked project is there because the specs browse as
    // an admin.
    const rows = await screen.findAllByRole("row");
    expect(
      rows.slice(1).map((r) => r.getAttribute("data-testid")),
    ).toEqual([
      "set-row-checkout-agent",
      "set-row-search-rerank",
      "set-row-support-bot",
    ]);
  });

  it("marks a locked project as restricted, and leaves an open one unmarked", async () => {
    renderSets();
    const locked = await screen.findByTestId("set-row-support-bot");
    expect(within(locked).getByText("restricted")).toBeInTheDocument();

    // `search-rerank` has a SUITE-level lock only; drawing its project row from
    // the covering answer would wrongly report the whole project as closed.
    const open = screen.getByTestId("set-row-search-rerank");
    expect(within(open).queryByText("restricted")).toBeNull();
  });

  it("teaches how sets come to exist when the caller can see none", async () => {
    // Everything locked, and this caller holds no grant: the listing is empty
    // because of policy, but "no sets" is useless without saying what makes one.
    for (const project of ["checkout-agent", "search-rerank", "support-bot"]) {
      fx.restrictSet(project, null);
    }
    fx.deleteGrant("support-bot", null, "u_member");
    fx.deleteGrant("search-rerank", "ndcg-eval", "u_member");
    login("member", "member");

    renderSets();
    expect(await screen.findByText(/No run sets yet/i)).toBeInTheDocument();
    expect(screen.getByText(/project:/)).toBeInTheDocument();
  });
});
