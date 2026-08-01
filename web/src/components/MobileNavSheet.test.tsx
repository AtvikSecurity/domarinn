import { beforeAll, describe, expect, it } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { MobileNavSheet } from "./MobileNavSheet";
import type { NavItem } from "@/lib/nav";

// Radix uses pointer capture and measurement that jsdom does not implement.
beforeAll(() => {
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.scrollIntoView = () => {};
  if (!("ResizeObserver" in globalThis)) {
    globalThis.ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

const NAV: NavItem[] = [
  { to: "/", label: "Overview", end: true },
  { to: "/runs", label: "Runs" },
  { to: "/sets", label: "Sets" },
];

function LocationProbe() {
  return <div data-testid="location">{useLocation().pathname}</div>;
}

function renderSheet(nav: NavItem[] = NAV) {
  return render(
    <MemoryRouter initialEntries={["/runs"]}>
      <MobileNavSheet nav={nav}>
        {() => <input aria-label="Search" />}
      </MobileNavSheet>
      <Routes>
        <Route path="*" element={<LocationProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("MobileNavSheet", () => {
  it("renders nothing when there is nowhere to go", () => {
    // Closed mode with an anonymous visitor: every link would bounce to /login.
    renderSheet([]);
    expect(screen.queryByRole("button", { name: "Open menu" })).toBeNull();
  });

  it("opens to the full nav list", async () => {
    const user = userEvent.setup();
    renderSheet();
    await user.click(screen.getByRole("button", { name: "Open menu" }));

    const menu = await screen.findByRole("navigation", { name: "Main menu" });
    for (const item of NAV) {
      expect(
        screen.getByRole("link", { name: item.label }),
      ).toBeInTheDocument();
    }
    expect(menu).toBeInTheDocument();
  });

  it("does not steal focus into the search box on open", async () => {
    const user = userEvent.setup();
    renderSheet();
    await user.click(screen.getByRole("button", { name: "Open menu" }));
    await screen.findByRole("navigation", { name: "Main menu" });

    // On a phone that would raise the keyboard over the list the menu exists
    // to show.
    expect(document.activeElement).not.toBe(
      screen.getByRole("textbox", { name: "Search" }),
    );
  });

  it("navigates and closes itself", async () => {
    const user = userEvent.setup();
    renderSheet();
    await user.click(screen.getByRole("button", { name: "Open menu" }));
    await user.click(await screen.findByRole("link", { name: "Sets" }));

    await waitFor(() =>
      expect(screen.getByTestId("location").textContent).toBe("/sets"),
    );
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    renderSheet();
    await user.click(screen.getByRole("button", { name: "Open menu" }));
    await screen.findByRole("dialog");

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});
