import { describe, expect, it } from "vitest";
import { canPinBaseline } from "./baselineAccess";

const reader = { canWrite: false, canAdmin: false };
const member = { canWrite: true, canAdmin: false };
const admin = { canWrite: true, canAdmin: true };
const open = { restricted: false, my_level: null };

describe("canPinBaseline", () => {
  it("refuses a read-scoped caller whatever the set", () => {
    expect(canPinBaseline(reader, open)).toBe(false);
    expect(canPinBaseline(reader, { restricted: true, my_level: "manage" })).toBe(false);
  });

  it("lets a writer pin an unrestricted set", () => {
    expect(canPinBaseline(member, open)).toBe(true);
  });

  it("needs an upload grant or better on a restricted set", () => {
    expect(canPinBaseline(member, { restricted: true, my_level: null })).toBe(false);
    expect(canPinBaseline(member, { restricted: true, my_level: "view" })).toBe(false);
    expect(canPinBaseline(member, { restricted: true, my_level: "upload" })).toBe(true);
    expect(canPinBaseline(member, { restricted: true, my_level: "manage" })).toBe(true);
  });

  it("lets an admin through the set gate without a grant", () => {
    expect(canPinBaseline(admin, { restricted: true, my_level: null })).toBe(true);
  });

  it("hides the control while the set loads, and defers to the server if it failed", () => {
    expect(canPinBaseline(member, undefined)).toBe(false);
    expect(canPinBaseline(member, null)).toBe(true);
  });
});
