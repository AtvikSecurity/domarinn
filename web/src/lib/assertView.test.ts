import { describe, expect, it } from "vitest";
import {
  criteriaView,
  formatThreshold,
  hasProseCriteria,
  verdictSource,
} from "./assertView";

describe("criteriaView", () => {
  it("reduces a typical llm-rubric to its rubric text plus a header threshold", () => {
    expect(
      criteriaView({
        type: "llm-rubric",
        value: "The response should name the target.",
        threshold: 0.7,
      }),
    ).toEqual({
      threshold: 0.7,
      negated: false,
      body: { kind: "scalar", text: "The response should name the target." },
    });
  });

  it("keeps a lone substring as a bare scalar", () => {
    expect(criteriaView({ type: "contains", value: "refund policy" })).toEqual({
      threshold: null,
      negated: false,
      body: { kind: "scalar", text: "refund policy" },
    });
  });

  it("keeps a named field's name, because the name is the meaning", () => {
    // Regression: reducing this to a bare "4000" rendered a `tokens` criteria
    // block that stated a number and withheld what it was a limit on.
    expect(criteriaView({ type: "tokens", max: 4000 })).toEqual({
      threshold: null,
      negated: false,
      body: { kind: "pairs", pairs: [["max", "4000"]] },
    });
  });

  it("lists several named scalar fields as pairs", () => {
    expect(criteriaView({ type: "length", min: 10, max: 200 })).toEqual({
      threshold: null,
      negated: false,
      body: {
        kind: "pairs",
        pairs: [
          ["min", "10"],
          ["max", "200"],
        ],
      },
    });
  });

  it("reports a negated assertion", () => {
    expect(
      criteriaView({ type: "contains", value: "refund policy", negate: true }),
    ).toMatchObject({
      negated: true,
      body: { kind: "scalar", text: "refund policy" },
    });
  });

  it("decomposes a structured field rather than stringifying it", () => {
    // `String({})` would render "[object Object]" on screen.
    expect(criteriaView({ type: "contains-json", schema: { a: 1 } })).toEqual({
      threshold: null,
      negated: false,
      body: { kind: "json", data: { schema: { a: 1 } } },
    });
  });

  it("decomposes a list of alternatives", () => {
    expect(criteriaView({ type: "icontains-any", values: ["a", "b"] })).toEqual({
      threshold: null,
      negated: false,
      body: { kind: "json", data: { values: ["a", "b"] } },
    });
  });

  it("decomposes a top-level list criterion", () => {
    expect(criteriaView(["alpha", "beta"])).toEqual({
      threshold: null,
      negated: false,
      body: { kind: "json", data: ["alpha", "beta"] },
    });
  });

  it("does not print the word null as if it were the criterion", () => {
    expect(criteriaView({ type: "contains", value: null })).toEqual({
      threshold: null,
      negated: false,
      body: { kind: "json", data: { value: null } },
    });
  });

  it("returns null when the kind is the whole assertion", () => {
    // `is-json` has nothing to say beyond its name; a labelled empty block is
    // worse than no block.
    expect(criteriaView({ type: "is-json" })).toBeNull();
    expect(criteriaView(null)).toBeNull();
    expect(criteriaView(undefined)).toBeNull();
  });

  it("still reports a threshold or negation when no body remains", () => {
    expect(criteriaView({ type: "similar", threshold: 0.9 })).toEqual({
      threshold: 0.9,
      negated: false,
      body: null,
    });
    expect(criteriaView({ type: "is-json", negate: true })).toMatchObject({
      negated: true,
      body: null,
    });
  });

  it("ignores a non-numeric threshold instead of printing NaN", () => {
    expect(
      criteriaView({ type: "llm-rubric", value: "x", threshold: "high" }),
    ).toMatchObject({ threshold: null, body: { kind: "scalar", text: "x" } });
    expect(
      criteriaView({ type: "llm-rubric", value: "x", threshold: NaN }),
    ).toMatchObject({ threshold: null });
  });

  it("handles a bare scalar criteria blob", () => {
    expect(criteriaView("just a string")).toMatchObject({
      body: { kind: "scalar", text: "just a string" },
    });
  });
});

describe("hasProseCriteria", () => {
  it("treats rubric and similarity criteria as prose", () => {
    expect(hasProseCriteria("llm-rubric")).toBe(true);
    expect(hasProseCriteria("similar")).toBe(true);
  });

  it("treats character-exact criteria as mono", () => {
    // Whitespace and punctuation are part of these assertions, so the typeface
    // carries information rather than style.
    for (const kind of ["contains", "regex", "equals", "jinja", "length"] as const) {
      expect(hasProseCriteria(kind)).toBe(false);
    }
  });
});

describe("verdictSource", () => {
  it("names a grader verdict as model-written", () => {
    expect(verdictSource("llm-rubric")).toEqual({
      label: "Grader verdict",
      hint: "written by the grading model, not measured",
    });
  });

  it("attributes an exec assertion to the user's script", () => {
    expect(verdictSource("exec").label).toBe("Script result");
  });

  it("labels a deterministic check plainly, with no provenance note", () => {
    expect(verdictSource("contains")).toEqual({ label: "Result" });
    expect(verdictSource("tokens").hint).toBeUndefined();
  });
});

describe("formatThreshold", () => {
  it("reads as the bar the score had to clear", () => {
    expect(formatThreshold(0.7)).toBe("needs ≥ 0.70");
  });

  it("is null when there is no threshold", () => {
    expect(formatThreshold(null)).toBeNull();
  });
});
