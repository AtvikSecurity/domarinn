import { describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { RunsList } from "./RunsList";

function renderRunsList() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    // `TooltipProvider` mirrors the app root: the run rows use real tooltips for
    // the absolute timestamp and the full commit sha, and Radix throws without
    // a provider ancestor.
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/"]}>
          <RunsList />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

/**
 * These two mount the whole runs list against the mock API, click two rows and
 * then ask for a link by accessible name — a document-wide name computation
 * over every row, tooltip and chip on the page. That is ~1.3s of real work on
 * an idle machine and ~5.6s when the other 63 test files are competing for
 * cores, which put it either side of vitest's 5s default at random.
 *
 * The budget is raised rather than the work reduced: the cost IS the page these
 * assertions are about, and querying by test id instead would stop proving the
 * link is reachable the way a user reaches it.
 */
const HEAVY = { timeout: 20_000 };

// Regression pin for the compare-link bug cluster: the real server route is
// `GET /runs/{id}/compare/{other}` -> `{ base: id, head: other }` (first url
// segment is always base). A baseline comparison wants the OLDER run as
// base, so any link RunsList builds must put the older selected run first.
describe("RunsList compare-2-runs link", () => {
  it("places the older selected run in the first (base) url segment, newer in the second (head)", HEAVY, async () => {
    const user = userEvent.setup();
    renderRunsList();

    // checkout-agent/regression run ids are zero-padded and chronological
    // (see src/mocks/fixtures.ts) — "-11" is older than "-12".
    await screen.findByLabelText("Select run checkout-agent-regression-12");

    await user.click(screen.getByLabelText("Select run checkout-agent-regression-11"));
    await user.click(screen.getByLabelText("Select run checkout-agent-regression-12"));

    const link = await screen.findByRole("link", { name: "Compare 2 runs" });
    expect(link).toHaveAttribute(
      "href",
      "/runs/checkout-agent-regression-11/compare/checkout-agent-regression-12",
    );
  });

  it("still puts the older run first regardless of click order", HEAVY, async () => {
    const user = userEvent.setup();
    renderRunsList();

    await screen.findByLabelText("Select run checkout-agent-regression-12");

    // Click the newer run first this time.
    await user.click(screen.getByLabelText("Select run checkout-agent-regression-12"));
    await user.click(screen.getByLabelText("Select run checkout-agent-regression-11"));

    const link = await screen.findByRole("link", { name: "Compare 2 runs" });
    expect(link).toHaveAttribute(
      "href",
      "/runs/checkout-agent-regression-11/compare/checkout-agent-regression-12",
    );
  });
});
