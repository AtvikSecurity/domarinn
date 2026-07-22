import { describe, expect, it } from "vitest";
import { detectContent, outputToString } from "./detect";

describe("detectContent", () => {
  describe("json", () => {
    it("classifies a structured object value", () => {
      expect(detectContent({ a: 1 }).type).toBe("json");
    });

    it("classifies a structured array value", () => {
      expect(detectContent([1, 2, 3]).type).toBe("json");
    });

    it("classifies a JSON object string", () => {
      expect(detectContent('{"intent":"resolve","n":2}').type).toBe("json");
    });

    it("classifies a JSON array string", () => {
      expect(detectContent("[1, 2, 3]").type).toBe("json");
    });

    it("classifies a JSON string with leading/trailing whitespace", () => {
      expect(detectContent('\n  { "ok": true }\n').type).toBe("json");
    });

    it("does NOT treat a bare JSON number as json", () => {
      // Valid JSON, but not an object/array — there's nothing to tree-render.
      expect(detectContent("42").type).toBe("text");
    });

    it("does NOT treat a bare JSON string literal as json", () => {
      expect(detectContent('"just a quoted string"').type).toBe("text");
    });
  });

  describe("markdown", () => {
    it("detects an ATX heading", () => {
      expect(detectContent("# Title\n\nbody").type).toBe("markdown");
    });

    it("detects a fenced code block and extracts the lang hint", () => {
      const d = detectContent("Here:\n\n```python\nprint(1)\n```\n");
      expect(d.type).toBe("markdown");
      expect(d.langHint).toBe("python");
    });

    it("detects a bullet list of two or more items", () => {
      expect(detectContent("- one\n- two\n- three").type).toBe("markdown");
    });

    it("detects an inline link", () => {
      expect(detectContent("see [the docs](https://x.example) now").type).toBe(
        "markdown",
      );
    });

    it("detects bold emphasis", () => {
      expect(detectContent("this is **very** important").type).toBe("markdown");
    });

    it("detects a blockquote", () => {
      expect(detectContent("> quoted wisdom here").type).toBe("markdown");
    });

    it("detects a GFM table", () => {
      const md = "| a | b |\n| --- | --- |\n| 1 | 2 |";
      expect(detectContent(md).type).toBe("markdown");
    });
  });

  describe("text (avoids false positives)", () => {
    it("classifies plain prose as text", () => {
      expect(
        detectContent(
          "The agent resolved the request and returned a structured result.",
        ).type,
      ).toBe("text");
    });

    it("does not treat a hyphen mid-sentence as a list", () => {
      expect(detectContent("well - maybe it works out fine").type).toBe("text");
    });

    it("does not treat arithmetic asterisks as emphasis", () => {
      expect(detectContent("compute 2 * 3 * 4 for the total").type).toBe("text");
    });

    it("classifies invalid JSON-looking prose as text", () => {
      expect(detectContent("{this is not valid json}").type).toBe("text");
    });

    it("classifies a { that never closes as text", () => {
      expect(detectContent("{ unterminated").type).toBe("text");
    });

    it("treats an empty / whitespace string as text", () => {
      expect(detectContent("   ").type).toBe("text");
      expect(detectContent("").type).toBe("text");
    });

    it("treats null/undefined as text", () => {
      expect(detectContent(null).type).toBe("text");
      expect(detectContent(undefined).type).toBe("text");
    });
  });
});

describe("outputToString", () => {
  it("passes strings through untouched", () => {
    expect(outputToString("hello")).toBe("hello");
  });

  it("pretty-prints structured values", () => {
    expect(outputToString({ a: 1 })).toBe('{\n  "a": 1\n}');
  });

  it("returns an empty string for null/undefined", () => {
    expect(outputToString(null)).toBe("");
    expect(outputToString(undefined)).toBe("");
  });
});
