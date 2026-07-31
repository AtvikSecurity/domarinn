import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { AuthProvider } from "@/auth/AuthProvider";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { login, resetMockAuth } from "@/mocks/authState";
import * as fx from "@/mocks/fixtures";
import { SetProjectPage } from "./SetProjectPage";

beforeEach(() => {
  resetMockAuth();
  fx.resetSets();
});

function renderProject(project: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <TooltipProvider>
          <MemoryRouter initialEntries={[`/sets/${project}`]}>
            <Routes>
              <Route path="/sets/:project" element={<SetProjectPage />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

describe("SetProjectPage", () => {
  it("links each suite of the project", async () => {
    renderProject("checkout-agent");
    expect(await screen.findByRole("link", { name: "regression" })).toHaveAttribute(
      "href",
      "/sets/checkout-agent/regression",
    );
    expect(screen.getByRole("link", { name: "canary" })).toBeInTheDocument();
  });

  it("flags the suite whose own lock is what restricts it", async () => {
    // `search-rerank` is open; only `ndcg-eval` inside it is restricted.
    renderProject("search-rerank");
    const locked = await screen.findByTestId("suite-row-ndcg-eval");
    expect(within(locked).getByText("restricted")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /restricted/ })).toBeNull();
  });

  it("does not repeat the project's own lock on every suite row", async () => {
    // `support-bot` is locked at the project level, so its suites report
    // restricted=true (the COVERING answer) while none of them owns a lock.
    // Drawing the row chip from that would say "restricted" on every line.
    renderProject("support-bot");
    const row = await screen.findByTestId("suite-row-faq-accuracy");
    expect(within(row).queryByText("restricted")).toBeNull();
    // The heading says it once, where the fact actually lives.
    expect(screen.getAllByText("restricted")).toHaveLength(1);
  });

  it("offers the access panel to a manage-grant holder", async () => {
    login("member", "member");
    renderProject("support-bot");
    expect(
      await screen.findByRole("button", { name: "Access" }),
    ).toBeInTheDocument();
  });

  it("hides the access panel from someone who only holds a view grant", async () => {
    login("member", "member");
    renderProject("search-rerank");
    await screen.findByRole("link", { name: "ndcg-eval" });
    expect(screen.queryByRole("button", { name: "Access" })).toBeNull();
  });
});
