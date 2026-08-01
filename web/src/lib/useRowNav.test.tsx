import { describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes, useLocation } from "react-router";
import { useRowNav } from "./useRowNav";

function LocationProbe() {
  const location = useLocation();
  return <div data-testid="location">{location.pathname}</div>;
}

/** A row shaped like the real ones: a checkbox, a link, and a plain cell. */
function Harness() {
  const rowNav = useRowNav();
  return (
    <table>
      <tbody>
        <tr data-testid="row" {...rowNav("/runs/abc")}>
          <td>
            <label>
              <input type="checkbox" aria-label="Select run" />
            </label>
          </td>
          <td>
            <Link to="/runs/abc">abc</Link>
          </td>
          <td>
            <button type="button" onClick={() => undefined}>
              Copy
            </button>
          </td>
          <td data-testid="plain-cell">2 hours ago</td>
        </tr>
      </tbody>
    </table>
  );
}

function renderRow() {
  return render(
    <MemoryRouter initialEntries={["/runs"]}>
      <Harness />
      <Routes>
        <Route path="*" element={<LocationProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

const at = () => screen.getByTestId("location").textContent;

describe("useRowNav", () => {
  it("navigates when a plain cell is clicked", async () => {
    const user = userEvent.setup();
    renderRow();
    await user.click(screen.getByTestId("plain-cell"));
    expect(at()).toBe("/runs/abc");
  });

  it("leaves the checkbox alone", async () => {
    const user = userEvent.setup();
    renderRow();
    const box = screen.getByRole("checkbox");
    await user.click(box);
    // Selecting a run for comparison must not navigate away from the list.
    expect(box).toBeChecked();
    expect(at()).toBe("/runs");
  });

  it("leaves a button in the row alone", async () => {
    const user = userEvent.setup();
    renderRow();
    // Without the `closest()` guard this would copy *and* navigate.
    await user.click(screen.getByRole("button", { name: "Copy" }));
    expect(at()).toBe("/runs");
  });

  it("lets the cell link navigate on its own, exactly once", async () => {
    const user = userEvent.setup();
    renderRow();
    await user.click(screen.getByRole("link", { name: "abc" }));
    expect(at()).toBe("/runs/abc");
  });

  it("ignores modified clicks so the browser can open a new tab", () => {
    renderRow();
    const cell = screen.getByTestId("plain-cell");
    for (const modifier of [
      { metaKey: true },
      { ctrlKey: true },
      { shiftKey: true },
      { altKey: true },
    ]) {
      fireEvent.mouseDown(cell, { clientX: 10, clientY: 10 });
      fireEvent.click(cell, { clientX: 10, clientY: 10, ...modifier });
      expect(at()).toBe("/runs");
    }
  });

  it("ignores a drag, so selecting text or scrolling sideways is not a click", () => {
    renderRow();
    const cell = screen.getByTestId("plain-cell");
    fireEvent.mouseDown(cell, { clientX: 10, clientY: 10 });
    fireEvent.click(cell, { clientX: 60, clientY: 12 });
    expect(at()).toBe("/runs");
  });

  it("still navigates when the pointer barely moves", () => {
    renderRow();
    const cell = screen.getByTestId("plain-cell");
    fireEvent.mouseDown(cell, { clientX: 10, clientY: 10 });
    fireEvent.click(cell, { clientX: 12, clientY: 11 });
    expect(at()).toBe("/runs/abc");
  });
});
