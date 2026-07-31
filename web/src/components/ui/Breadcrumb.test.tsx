import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { Breadcrumb } from "./Breadcrumb";

function renderTrail() {
  return render(
    <MemoryRouter>
      <Breadcrumb
        items={[
          { label: "Sets", to: "/sets" },
          { label: "checkout-agent", to: "/sets/checkout-agent" },
          { label: "regression" },
        ]}
      />
    </MemoryRouter>,
  );
}

describe("Breadcrumb", () => {
  it("is a landmark a screen reader can jump to by name", () => {
    renderTrail();
    expect(
      screen.getByRole("navigation", { name: "Breadcrumb" }),
    ).toBeInTheDocument();
  });

  it("links every ancestor", () => {
    renderTrail();
    expect(screen.getByRole("link", { name: "Sets" })).toHaveAttribute(
      "href",
      "/sets",
    );
    expect(screen.getByRole("link", { name: "checkout-agent" })).toHaveAttribute(
      "href",
      "/sets/checkout-agent",
    );
  });

  it("marks the last crumb as the current page rather than linking it", () => {
    // A link to the page you are already on is the classic breadcrumb bug:
    // it reads as somewhere else to go and announces as an unvisited target.
    renderTrail();
    const current = screen.getByText("regression");
    expect(current).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("link", { name: "regression" })).toBeNull();
  });

  it("hides the separators from assistive tech", () => {
    // Two crumbs, one separator — announced, it would read as a URL fragment.
    const { container } = renderTrail();
    const separators = container.querySelectorAll('[aria-hidden="true"]');
    expect(separators).toHaveLength(2);
    expect(separators[0]).toHaveTextContent("/");
  });
});
