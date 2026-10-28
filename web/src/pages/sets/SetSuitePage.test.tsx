import { beforeEach, describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { AuthProvider } from "@/auth/AuthProvider";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { createUser, login, resetMockAuth } from "@/mocks/authState";
import * as fx from "@/mocks/fixtures";
import { SetSuitePage } from "./SetSuitePage";

const REGRESSION = "checkout-agent/regression";
let seededPin: string | undefined;

beforeEach(() => {
  resetMockAuth();
  fx.resetSets();
  // Unpinning deletes the seeded run pin, which no reset restores.
  seededPin ??= fx.suiteBaseline(REGRESSION);
  if (seededPin) fx.setSuiteBaseline("checkout-agent", "regression", seededPin);
});

function renderSuite(project: string, suite: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <TooltipProvider>
          <MemoryRouter initialEntries={[`/sets/${project}/${suite}`]}>
            <Routes>
              <Route path="/sets/:project/:suite" element={<SetSuitePage />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

describe("SetSuitePage baseline controls", () => {
  it("hides pin and unpin from a read-only viewer", async () => {
    // Open mode lets anyone write, so the read-only case needs a mode that
    // does not. `resetMockAuth` clears the flag again.
    localStorage.setItem("domarinn.mock.authmode", "protect-writes");
    createUser("reader", "readerpass", "viewer");
    login("reader", "readerpass");
    renderSuite("checkout-agent", "regression");
    await screen.findByRole("heading", { name: "regression" });
    expect(screen.queryByRole("button", { name: /Pin branch/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Unpin baseline" })).toBeNull();
  });

  it("hides them on a restricted set where the caller holds only a view grant", async () => {
    login("member", "member");
    renderSuite("search-rerank", "ndcg-eval");
    await screen.findByRole("heading", { name: "ndcg-eval" });
    expect(screen.queryByRole("button", { name: /Pin branch/ })).toBeNull();
  });

  it("offers them on a restricted set where the caller can upload", async () => {
    // `member` manages support-bot, and manage includes upload.
    login("member", "member");
    renderSuite("support-bot", "faq-accuracy");
    expect(
      await screen.findByRole("button", { name: /Pin branch/ }),
    ).toBeInTheDocument();
  });

  it("opens the branch field on the branch already pinned", async () => {
    const user = userEvent.setup();
    fx.pinSuiteBaselineBranch("checkout-agent", "regression", "main");
    renderSuite("checkout-agent", "regression");
    await user.click(await screen.findByRole("button", { name: /Pin branch/ }));
    expect(screen.getByLabelText("Baseline branch")).toHaveValue("main");
  });

  it("unpins the baseline after confirming", async () => {
    const user = userEvent.setup();
    fx.pinSuiteBaselineBranch("checkout-agent", "regression", "main");
    renderSuite("checkout-agent", "regression");
    expect(await screen.findByText("baseline: main")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Unpin baseline" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("branch main")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Unpin baseline" }));

    await waitFor(() => expect(screen.queryByText("baseline: main")).toBeNull());
    expect(screen.queryByRole("dialog")).toBeNull();
    // Nothing pinned, so nothing to unpin.
    expect(screen.queryByRole("button", { name: "Unpin baseline" })).toBeNull();
  });
});
