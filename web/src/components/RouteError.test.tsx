import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { RouteError } from "./RouteError";

function Boom(): never {
  throw new Error("tags is undefined somewhere deep");
}

describe("RouteError", () => {
  // react-router logs the caught error; keep test output clean.
  beforeEach(() => {
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("renders a recoverable error screen instead of the router default", () => {
    const router = createMemoryRouter([
      { path: "/", element: <Boom />, errorElement: <RouteError /> },
    ]);
    render(<RouterProvider router={router} />);

    expect(screen.getByText("Something went wrong")).toBeInTheDocument();
    expect(
      screen.getByText(/tags is undefined somewhere deep/),
    ).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to runs" })).toHaveAttribute(
      "href",
      "/",
    );
  });
});
