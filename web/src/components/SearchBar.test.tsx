import { describe, expect, it } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { SearchBar } from "./SearchBar";

function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location">{location.pathname + location.search}</div>;
}

function renderBar() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/"]}>
        <SearchBar />
        <Routes>
          <Route path="*" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("SearchBar", () => {
  it("shows a grouped dropdown after typing (debounced) and navigates on click", async () => {
    const user = userEvent.setup();
    renderBar();

    const input = screen.getByRole("combobox", { name: "Search runs and cases" });
    await user.type(input, "checkout");

    // Debounce (200ms) then the mock responds; the group label appears
    // ("checkout" matches run metadata only, so just the Runs group).
    expect(await screen.findByText("Runs")).toBeInTheDocument();

    // Click the first hit: a run hit navigates to its run page. Selected by
    // `data-search-hit`, not by row copy: matching on text meant the assertion
    // silently depended on which groups the dropdown happened to render.
    const options = document.querySelectorAll<HTMLElement>(
      '[data-search-hit="run"]',
    );
    expect(options.length).toBeGreaterThan(0);
    await user.click(options[0]!);
    await waitFor(() =>
      expect(screen.getByTestId("location").textContent).toMatch(/^\/runs\//),
    );
  });

  it("Enter with no selection opens the full /search page", async () => {
    const user = userEvent.setup();
    renderBar();

    const input = screen.getByRole("combobox", { name: "Search runs and cases" });
    await user.type(input, "checkout{Enter}");

    await waitFor(() =>
      expect(screen.getByTestId("location").textContent).toBe(
        "/search?q=checkout",
      ),
    );
  });

  it("Escape closes the dropdown", async () => {
    const user = userEvent.setup();
    renderBar();

    const input = screen.getByRole("combobox", { name: "Search runs and cases" });
    await user.type(input, "checkout");
    expect(await screen.findByText("Runs")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByText("Runs")).not.toBeInTheDocument();
  });
});
