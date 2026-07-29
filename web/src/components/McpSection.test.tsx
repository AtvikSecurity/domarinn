import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { McpSection } from "./McpSection";

function renderSection(enabled: boolean) {
  return render(
    <MemoryRouter>
      <McpSection enabled={enabled} />
    </MemoryRouter>,
  );
}

describe("McpSection", () => {
  it("shows the endpoint and a connect command when enabled", () => {
    renderSection(true);
    expect(screen.getByText(/Accepting connections/)).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/api/v1/mcp`)).toBeInTheDocument();
    expect(
      screen.getByText(/claude mcp add --transport http domarinn/),
    ).toBeInTheDocument();
  });

  /// The token is never interpolated into the shown command: a real secret in a
  /// copy-pasted shell line lands in the operator's history.
  it("references the token by variable rather than embedding one", () => {
    renderSection(true);
    const command = screen.getByText(/claude mcp add/).textContent ?? "";
    expect(command).toContain("$DOMARINN_TOKEN");
    expect(command).not.toMatch(/Bearer domarinn_[0-9a-f]/);
  });

  it("states that the surface is read-only", () => {
    renderSection(true);
    expect(screen.getByText(/no\s+tool can start a run/i)).toBeInTheDocument();
  });

  /// Disabled and misconfigured are indistinguishable over HTTP (both are a
  /// JSON 404), so the disabled state has to name the fix itself.
  it("names the variable that turns it on when disabled", () => {
    renderSection(false);
    expect(screen.getByText(/Not enabled\./)).toBeInTheDocument();
    expect(screen.getByText("DOMARINN_MCP_ENABLED=true")).toBeInTheDocument();
    expect(screen.queryByText(/claude mcp add/)).not.toBeInTheDocument();
  });

  it("offers a copy control for every command it shows", () => {
    renderSection(true);
    expect(screen.getByLabelText("Copy endpoint")).toBeInTheDocument();
    expect(screen.getByLabelText("Copy command")).toBeInTheDocument();
  });
});
