import { describe, expect, it } from "vitest";
import {
  authTokenReducer,
  initialAuthTokenState,
  type AuthTokenState,
} from "./reducer";

describe("authTokenReducer", () => {
  it("seeds from an existing token", () => {
    expect(initialAuthTokenState("abc")).toEqual({ token: "abc" });
    expect(initialAuthTokenState(null)).toEqual({ token: null });
  });

  it("signed-in sets the token", () => {
    const next = authTokenReducer({ token: null }, { type: "signed-in", token: "t1" });
    expect(next.token).toBe("t1");
  });

  it("signed-out clears the token", () => {
    const next = authTokenReducer({ token: "t1" }, { type: "signed-out" });
    expect(next.token).toBeNull();
  });

  it("sync reconciles with an external value", () => {
    const next = authTokenReducer({ token: "old" }, { type: "sync", token: "new" });
    expect(next.token).toBe("new");
  });

  it("returns the same reference when nothing changes (stable renders)", () => {
    const state: AuthTokenState = { token: "same" };
    expect(authTokenReducer(state, { type: "signed-in", token: "same" })).toBe(state);
    expect(authTokenReducer(state, { type: "sync", token: "same" })).toBe(state);
    const cleared: AuthTokenState = { token: null };
    expect(authTokenReducer(cleared, { type: "signed-out" })).toBe(cleared);
  });
});
