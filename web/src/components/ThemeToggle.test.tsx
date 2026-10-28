import { beforeEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ThemeSegmented } from "./ThemeToggle";

beforeEach(() => {
  localStorage.clear();
  document.documentElement.classList.remove("dark");
});

function radio(name: string) {
  return screen.getByRole("radio", { name });
}

describe("ThemeSegmented", () => {
  it("is a single Tab stop on the selected option", async () => {
    const user = userEvent.setup();
    render(
      <>
        <button type="button">before</button>
        <ThemeSegmented />
        <button type="button">after</button>
      </>,
    );
    // No stored preference means "system".
    expect(radio("System")).toHaveAttribute("aria-checked", "true");
    expect(radio("System")).toHaveAttribute("tabindex", "0");
    expect(radio("Light")).toHaveAttribute("tabindex", "-1");
    expect(radio("Dark")).toHaveAttribute("tabindex", "-1");

    await user.click(screen.getByRole("button", { name: "before" }));
    await user.tab();
    expect(radio("System")).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "after" })).toHaveFocus();
  });

  it("moves selection and focus with arrows, wrapping at the ends", async () => {
    const user = userEvent.setup();
    render(<ThemeSegmented />);
    await user.click(radio("Light"));
    expect(radio("Light")).toHaveAttribute("aria-checked", "true");

    await user.keyboard("{ArrowRight}");
    expect(radio("Dark")).toHaveFocus();
    expect(radio("Dark")).toHaveAttribute("aria-checked", "true");
    expect(radio("Dark")).toHaveAttribute("tabindex", "0");
    expect(radio("Light")).toHaveAttribute("tabindex", "-1");
    expect(document.documentElement).toHaveClass("dark");

    await user.keyboard("{ArrowDown}");
    expect(radio("System")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(radio("Light")).toHaveFocus();
    expect(radio("Light")).toHaveAttribute("aria-checked", "true");

    await user.keyboard("{ArrowLeft}");
    expect(radio("System")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(radio("Dark")).toHaveFocus();
    expect(radio("Dark")).toHaveAttribute("aria-checked", "true");
  });

  it("jumps to the first and last option with Home and End", async () => {
    const user = userEvent.setup();
    render(<ThemeSegmented />);
    await user.click(radio("Dark"));

    await user.keyboard("{End}");
    expect(radio("System")).toHaveFocus();
    expect(radio("System")).toHaveAttribute("aria-checked", "true");

    await user.keyboard("{Home}");
    expect(radio("Light")).toHaveFocus();
    expect(radio("Light")).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem("domarinn.theme")).toBe("light");
  });
});
