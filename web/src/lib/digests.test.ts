import { describe, expect, it } from "vitest";
import type { ComponentDrift } from "@/api";
import { changedComponents, componentLabel, hasUnknownComponents } from "./digests";

const drift = (component: string, changed: boolean | null): ComponentDrift => ({
  component,
  base: changed === null ? null : "blake3:a",
  head: changed === null ? null : changed ? "blake3:b" : "blake3:a",
  changed,
});

describe("changedComponents", () => {
  it("reports only what definitely changed", () => {
    expect(
      changedComponents([
        drift("prompts", true),
        drift("providers", false),
        drift("tests", false),
      ]),
    ).toEqual(["prompts"]);
  });

  // `null` means one side predates component digests. Reporting it as changed
  // would invent a finding; the caller shows nothing instead.
  it("never reports an unknown component as changed", () => {
    expect(changedComponents([drift("prompts", null), drift("providers", null)])).toEqual([]);
  });

  // Ordered so the component most likely to explain a difference comes first.
  it("orders by likelihood of explaining a change", () => {
    expect(
      changedComponents([
        drift("grader", true),
        drift("prompts", true),
        drift("providers", true),
      ]),
    ).toEqual(["prompts", "providers", "grader"]);
  });

  it("is empty when nothing moved", () => {
    expect(changedComponents([drift("prompts", false)])).toEqual([]);
  });
});

describe("hasUnknownComponents", () => {
  it("detects a comparison that cannot know", () => {
    expect(hasUnknownComponents([drift("prompts", null)])).toBe(true);
    expect(hasUnknownComponents([drift("prompts", false)])).toBe(false);
  });
});

describe("componentLabel", () => {
  // `providers` is the stored name; "model" is what anyone reading results
  // calls it.
  it("renames providers to model", () => {
    expect(componentLabel("providers")).toBe("model");
    expect(componentLabel("prompts")).toBe("prompts");
  });

  it("passes through an unrecognized component", () => {
    expect(componentLabel("something_new")).toBe("something_new");
  });
});
