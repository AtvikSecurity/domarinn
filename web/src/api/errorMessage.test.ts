import { describe, expect, it } from "vitest";
import { ApiError } from "./client";
import { apiErrorMessage } from "./errorMessage";

describe("apiErrorMessage", () => {
  it("prefers the server's own sentence", () => {
    const err = new ApiError(409, "409 Conflict", { error: "username already exists" });
    expect(apiErrorMessage(err, "Could not create the user.")).toBe("username already exists");
  });

  it("names a permission refusal that carries no sentence", () => {
    expect(apiErrorMessage(new ApiError(403, "403 Forbidden"), "Failed.")).toBe(
      "You don't have permission to do that.",
    );
  });

  it("falls back on a blank or non-string error", () => {
    expect(apiErrorMessage(new ApiError(400, "400", { error: "  " }), "Failed.")).toBe("Failed.");
    expect(apiErrorMessage(new ApiError(500, "500", { error: 7 }), "Failed.")).toBe("Failed.");
  });

  it("falls back on anything that is not an ApiError", () => {
    expect(apiErrorMessage(new TypeError("fetch failed"), "Failed.")).toBe("Failed.");
    expect(apiErrorMessage(undefined, "Failed.")).toBe("Failed.");
  });
});
