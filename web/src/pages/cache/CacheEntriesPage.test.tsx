import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { beforeAll, describe, expect, it } from "vitest";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { shortCacheKey } from "@/lib/format";
import * as fx from "@/mocks/fixtures";
import { CacheEntriesPage } from "./CacheEntriesPage";

// jsdom has no layout, so `useVirtualizer` sees a zero-height viewport and
// renders no rows at all — which is why the repo's other virtualized grid is
// covered only by Playwright. Two shims fix that here.
//
// The measurement one has to be `offsetWidth`/`offsetHeight` specifically:
// virtual-core's `getRect` reads those, not `getBoundingClientRect`, and jsdom
// hardcodes both to 0. Mocking the wrong one looks right and changes nothing.
beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;

  for (const [prop, value] of [
    ["offsetWidth", 1200],
    ["offsetHeight", 800],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, prop, {
      configurable: true,
      get: () => value,
    });
  }
});

function renderPage(initialEntry = "/cache/entries") {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[initialEntry]}>
          <CacheEntriesPage />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

/** The first fixture entry in whatever state the test cares about. */
function firstEntry(predicate: (e: { indexed: boolean }) => boolean) {
  const found = fx.cacheEntryList().find(predicate);
  if (!found) throw new Error("fixture is missing a required entry shape");
  return found;
}

async function rows() {
  const grid = await screen.findByRole("grid");
  return within(grid)
    .getAllByRole("row")
    .filter((r) => r.getAttribute("aria-rowindex") !== "1");
}

describe("CacheEntriesPage", () => {
  it("renders a row per entry with an announceable label", async () => {
    renderPage();
    const all = await rows();
    expect(all.length).toBeGreaterThan(0);
    // A truncated hash is not announceable on its own, so the row must say
    // what it is about.
    expect(all[0]).toHaveAttribute("aria-label", expect.stringContaining("Cache entry"));
  });

  it("opens the drawer for the clicked entry", async () => {
    const user = userEvent.setup();
    renderPage();
    const [first] = await rows();
    await user.click(first!);

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toBeInTheDocument();
  });

  it("says a row is still indexing rather than claiming it has no model", async () => {
    // The distinction the `indexed` flag exists for: "we have not looked yet"
    // is not the same statement as "there is nothing there".
    const unindexed = firstEntry((e) => !e.indexed);
    expect(unindexed.model).toBeNull();
    renderPage();
    await screen.findByRole("grid");
    expect(await screen.findAllByText("indexing…")).not.toHaveLength(0);
  });

  it("sorts on the server, so the largest entry leads — not just the loaded page", async () => {
    // The assertion that matters is the *identity* of the top row, not that
    // something changed: `sort` is a server key here, and a client-side sort
    // would order only the rows already loaded while looking identical.
    const largest = fx
      .cacheEntryList()
      .reduce((a, b) => (b.size > a.size ? b : a));

    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("grid");
    await user.click(screen.getByRole("button", { name: /Size/i }));

    await waitFor(async () => {
      const top = (await rows())[0]?.getAttribute("aria-label");
      expect(top).toContain(shortCacheKey(largest.key));
    });
  });

  it("shows the teaching empty state, not just 'no entries', when nothing matches", async () => {
    renderPage("/cache/entries?q=zzzznotpresentzzzz");
    expect(
      await screen.findByText(/No entries match these filters/i),
    ).toBeInTheDocument();
  });
});

describe("CacheEntryDrawer", () => {
  it("falls back to the provider fingerprint for an entry written before 0.5", async () => {
    const preRequest = fx
      .cacheEntryList()
      .find((e) => e.indexed && e.request_summary === null);
    expect(preRequest).toBeDefined();

    const user = userEvent.setup();
    renderPage(`/cache/entries?entry=${encodeURIComponent(preRequest!.key)}`);

    const dialog = await screen.findByRole("dialog");
    expect(
      await within(dialog).findByText(/predates request capture/i),
    ).toBeInTheDocument();
    await user.keyboard("{Escape}");
  });

  it("does not fetch raw provider metadata until it is asked for", async () => {
    // `raw` is the largest member of an entry; loading it on every drawer open
    // would cost the whole payload a second time.
    const user = userEvent.setup();
    const entry = fx.cacheEntryList().find((e) => e.parseable === true);
    renderPage(`/cache/entries?entry=${encodeURIComponent(entry!.key)}`);

    const dialog = await screen.findByRole("dialog");
    await user.click(
      await within(dialog).findByRole("button", { name: /Provider metadata/i }),
    );
    expect(
      await within(dialog).findByRole("button", { name: /Load raw metadata/i }),
    ).toBeInTheDocument();
  });

  it("does not look up runs until the section is expanded", async () => {
    // The one section whose answer costs a query against the runs database.
    const user = userEvent.setup();
    const linked = fx.cacheEntryList().find((e) => fx.cacheEntryRuns(e.key).cases.length > 0);
    renderPage(`/cache/entries?entry=${encodeURIComponent(linked!.key)}`);

    const dialog = await screen.findByRole("dialog");
    const toggle = await within(dialog).findByRole("button", { name: /Used by runs/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");

    await user.click(toggle);
    expect(await within(dialog).findByRole("link", { name: /refund policy/i })).toBeInTheDocument();
  });

  it("says an empty run list is not evidence the entry is unused", async () => {
    // A run only carries the key if it was recorded by a version that wrote
    // one, and no backfill can supply it — so silence has to be explained.
    const user = userEvent.setup();
    const unlinked = fx
      .cacheEntryList()
      .find((e) => e.parseable === true && fx.cacheEntryRuns(e.key).cases.length === 0);
    renderPage(`/cache/entries?entry=${encodeURIComponent(unlinked!.key)}`);

    const dialog = await screen.findByRole("dialog");
    await user.click(
      await within(dialog).findByRole("button", { name: /Used by runs/i }),
    );
    expect(
      await within(dialog).findByText(/not evidence that the entry is unused/i),
    ).toBeInTheDocument();
  });

  it("keeps a large entry's output collapsed on open", async () => {
    // Decided from the row's `size`, before the detail request lands — a 4 MiB
    // entry is 4 MiB of output, and auto-expanding it would hang the tab.
    const huge = fx
      .cacheEntryList()
      .reduce((a, b) => (b.size > a.size ? b : a));
    renderPage(`/cache/entries?entry=${encodeURIComponent(huge.key)}`);

    const dialog = await screen.findByRole("dialog");
    const toggle = await within(dialog).findByRole("button", { name: /Output/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
  });
});
