import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { expect, it } from "vitest";
import { runListItem } from "@/mocks/fixtures";
import { SuiteRunHistory } from "./SuiteRunHistory";

it("distinguishes an unscored run from a real zero pass rate", () => {
  const run = runListItem("checkout-agent-regression-12");
  render(
    <MemoryRouter>
      <SuiteRunHistory runs={[
        { ...run, id: "unscored", pass_count: 0, fail_count: 0, error_count: 0, xpass_count: 0 },
        { ...run, id: "zero", pass_count: 0, fail_count: 1, error_count: 0, xpass_count: 0 },
      ]} />
    </MemoryRouter>,
  );
  expect(screen.queryByRole("link", { name: /unscored/ })).not.toBeInTheDocument();
  expect(screen.getByRole("link", { name: /zero: 0\.0%/ })).toHaveAttribute("href", "/runs/zero");
  expect(screen.getByText(/1 run has no scored cases/)).toBeVisible();
});
