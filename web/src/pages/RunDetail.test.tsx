import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  onTestFinished,
  vi,
} from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { AuthProvider } from "@/auth/AuthProvider";
import { TooltipProvider } from "@/components/ui/Tooltip";
import { createUser, login, resetMockAuth } from "@/mocks/authState";
import * as fx from "@/mocks/fixtures";
import { mockFetch, setMockEnabled } from "@/mocks/handlers";
import { installVirtualizerShims } from "@/test/virtualizer";
import { RunDetail } from "./RunDetail";

// checkout-agent/regression's seeded baseline is the run before the newest.
const PINNED = "checkout-agent-regression-11";
const UNPINNED = "checkout-agent-regression-12";

beforeAll(() => installVirtualizerShims());

beforeEach(() => {
  resetMockAuth();
  fx.resetSets();
  fx.setSuiteBaseline("checkout-agent", "regression", PINNED);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  setMockEnabled(true);
});

function renderRun(id: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AuthProvider>
        <TooltipProvider>
          <MemoryRouter initialEntries={[`/runs/${id}`]}>
            <Routes>
              <Route path="/runs/:id" element={<RunDetail />} />
              <Route path="/" element={<p>home</p>} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </AuthProvider>
    </QueryClientProvider>,
  );
}

describe("RunDetail baseline controls", () => {
  it("offers Set baseline on a run that is not the pinned one", async () => {
    renderRun(UNPINNED);
    expect(
      await screen.findByRole("button", { name: "Set baseline" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Unpin baseline" })).toBeNull();
  });

  it("offers Unpin on the pinned run, and unpins after confirming", async () => {
    const user = userEvent.setup();
    renderRun(PINNED);
    await user.click(await screen.findByRole("button", { name: "Unpin baseline" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Unpin baseline" }));
    // The set detail refetches with nothing pinned, so the run can be pinned again.
    expect(
      await screen.findByRole("button", { name: "Set baseline" }),
    ).toBeInTheDocument();
    expect(fx.suiteBaseline("checkout-agent/regression")).toBeUndefined();
  });

  it("hides both from a read-only viewer", async () => {
    localStorage.setItem("domarinn.mock.authmode", "protect-writes");
    createUser("reader", "readerpass", "viewer");
    login("reader", "readerpass");
    renderRun(PINNED);
    await screen.findByRole("button", { name: "Download JSON" });
    expect(screen.queryByRole("button", { name: "Set baseline" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Unpin baseline" })).toBeNull();
  });
});

describe("RunDetail export", () => {
  it("saves the run document as a JSON file", async () => {
    const user = userEvent.setup();
    // jsdom has neither, so there is nothing to spy on; define and remove.
    const createObjectURL = vi.fn((_blob: Blob) => "blob:run");
    URL.createObjectURL = createObjectURL;
    URL.revokeObjectURL = vi.fn();
    onTestFinished(() => {
      Reflect.deleteProperty(URL, "createObjectURL");
      Reflect.deleteProperty(URL, "revokeObjectURL");
    });
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        expect(this.download).toBe(`${UNPINNED}.json`);
        expect(this.href).toBe("blob:run");
      });

    renderRun(UNPINNED);
    await user.click(await screen.findByRole("button", { name: "Download JSON" }));

    await waitFor(() => expect(click).toHaveBeenCalledTimes(1));
    const blob = createObjectURL.mock.calls[0]![0];
    expect(blob.type).toBe("application/json");
    const doc = JSON.parse(await blob.text()) as { id: string };
    expect(doc.id).toBe(UNPINNED);
  });
});

describe("RunDetail delete", () => {
  it("does not greet a reopened dialog with the previous attempt's failure", async () => {
    const user = userEvent.setup();
    // The fixture server always deletes; route DELETE to a failure instead.
    setMockEnabled(false);
    vi.stubGlobal("fetch", (url: string, init: RequestInit = {}) =>
      init.method === "DELETE"
        ? Promise.resolve(
            new Response(JSON.stringify({ error: "storage is read-only" }), {
              status: 500,
              headers: { "Content-Type": "application/json" },
            }),
          )
        : mockFetch(url, init),
    );

    renderRun(UNPINNED);
    await user.click(await screen.findByRole("button", { name: "Delete run" }));
    let dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete run" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "storage is read-only",
    );

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await user.click(screen.getByRole("button", { name: "Delete run" }));
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByRole("alert")).toBeNull();
  });
});
