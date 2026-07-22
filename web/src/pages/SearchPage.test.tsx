import { describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { SearchPage } from "./SearchPage";

function renderAt(url: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[url]}>
        <SearchPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("SearchPage", () => {
  it("prompts for a query when q is empty", () => {
    renderAt("/search");
    expect(screen.getByText(/Type in the search bar above/)).toBeInTheDocument();
  });

  it("renders run hits with highlighted snippets for run-metadata matches", async () => {
    // "checkout" appears only in run metadata (the checkout-agent project),
    // not in any case text — so exactly the Runs group renders.
    renderAt("/search?q=checkout");

    expect(await screen.findByText(/^Runs \(\d+\)$/)).toBeInTheDocument();
    expect(screen.queryByText(/^Cases \(\d+\)$/)).not.toBeInTheDocument();

    // The fixture snippet wraps the matched token in PUA markers, which the
    // Snippet component renders as <mark> highlights.
    const marks = document.querySelectorAll("mark");
    expect(marks.length).toBeGreaterThan(0);
    expect(marks[0]!.textContent?.toLowerCase()).toContain("checkout");
  });

  it("renders case hits that deep-link into the run's case drawer", async () => {
    // "coupon" comes from the fixture case vocabulary (names/outputs).
    renderAt("/search?q=coupon");

    expect(await screen.findByText(/^Cases \(\d+\)$/)).toBeInTheDocument();
    const links = screen.getAllByRole("link");
    expect(
      links.some((l) => /\/runs\/.+\?case=/.test(l.getAttribute("href") ?? "")),
    ).toBe(true);
  });

  it("shows an empty state when nothing matches", async () => {
    renderAt("/search?q=xyzzynothingmatchesthis");
    expect(await screen.findByText("No matches")).toBeInTheDocument();
  });
});
