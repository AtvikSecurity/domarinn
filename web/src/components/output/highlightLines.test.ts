import { describe, expect, it } from "vitest";
import hljs from "highlight.js/lib/core";
import python from "highlight.js/lib/languages/python";
import yaml from "highlight.js/lib/languages/yaml";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import { splitHighlightedLines } from "./highlightLines";

/**
 * The fixtures below are verbatim `hljs.highlight(...).value` output, not
 * hand-written approximations — the whole reason this module exists is that
 * highlight.js emits spans that straddle newlines, and a fixture that quietly
 * balanced them per line would test nothing.
 */
const PYTHON_DOCSTRING =
  '<span class="hljs-keyword">def</span> <span class="hljs-title function_">f</span>():\n' +
  '    <span class="hljs-string">&quot;&quot;&quot;Doc\n' +
  "    spans lines\n" +
  '    &quot;&quot;&quot;</span>\n' +
  '    <span class="hljs-keyword">return</span> <span class="hljs-number">1</span>';

const YAML_BLOCK_SCALAR =
  '<span class="hljs-attr">key:</span> <span class="hljs-string">|\n' +
  "  line one\n" +
  "  line two\n" +
  '</span><span class="hljs-attr">other:</span> <span class="hljs-number">1</span>';

/** Every line must stand alone: the gutter grid drops each into its own cell,
 *  so an unclosed tag would bleed its colour into every row below it. */
function isBalanced(line: string): boolean {
  let depth = 0;
  for (const tag of line.match(/<span\b[^>]*>|<\/span>/g) ?? []) {
    depth += tag === "</span>" ? -1 : 1;
    if (depth < 0) return false;
  }
  return depth === 0;
}

describe("splitHighlightedLines", () => {
  it("returns one entry for a single line, unchanged", () => {
    expect(splitHighlightedLines('<span class="hljs-number">1</span>')).toEqual([
      '<span class="hljs-number">1</span>',
    ]);
  });

  it("returns one empty line for empty input", () => {
    // Not `[]`: the gutter renders one row per entry, and a zero-length code
    // string is still one (empty) line.
    expect(splitHighlightedLines("")).toEqual([""]);
  });

  it("counts trailing and interior newlines as line breaks", () => {
    expect(splitHighlightedLines("a\nb")).toEqual(["a", "b"]);
    expect(splitHighlightedLines("\n")).toEqual(["", ""]);
    expect(splitHighlightedLines("a\n")).toEqual(["a", ""]);
  });

  it("closes and reopens a span that straddles a newline", () => {
    const lines = splitHighlightedLines(PYTHON_DOCSTRING);
    expect(lines).toEqual([
      '<span class="hljs-keyword">def</span> <span class="hljs-title function_">f</span>():',
      '    <span class="hljs-string">&quot;&quot;&quot;Doc</span>',
      '<span class="hljs-string">    spans lines</span>',
      '<span class="hljs-string">    &quot;&quot;&quot;</span>',
      '    <span class="hljs-keyword">return</span> <span class="hljs-number">1</span>',
    ]);
    expect(lines.every(isBalanced)).toBe(true);
  });

  it("does not leak an empty reopened span when a span's content ends in a newline", () => {
    // The yaml block scalar closes immediately after its final "\n". Reopening
    // eagerly would prefix the next line with `<span class="hljs-string"></span>`
    // — harmless to paint, but it makes the output impossible to assert on and
    // doubles the node count on long files.
    const lines = splitHighlightedLines(YAML_BLOCK_SCALAR);
    expect(lines).toEqual([
      '<span class="hljs-attr">key:</span> <span class="hljs-string">|</span>',
      '<span class="hljs-string">  line one</span>',
      '<span class="hljs-string">  line two</span>',
      '<span class="hljs-attr">other:</span> <span class="hljs-number">1</span>',
    ]);
    expect(lines.every(isBalanced)).toBe(true);
  });

  it("reopens nested spans outermost-first", () => {
    // hljs nests freely (`hljs-subst` inside `hljs-string` for a template
    // literal), so the reopen has to replay a stack, not a single tag.
    const lines = splitHighlightedLines(
      '<span class="a">one<span class="b">two\nthree</span>four</span>',
    );
    expect(lines).toEqual([
      '<span class="a">one<span class="b">two</span></span>',
      '<span class="a"><span class="b">three</span>four</span>',
    ]);
    expect(lines.every(isBalanced)).toBe(true);
  });

  it("preserves HTML entities byte-for-byte", () => {
    // The input is already-escaped markup on its way to dangerouslySetInnerHTML.
    // Decoding it anywhere in here would turn escaped source into live markup.
    const html = '<span class="hljs-string">&quot;a &amp; b&quot;</span>\n&lt;div&gt;';
    expect(splitHighlightedLines(html)).toEqual([
      '<span class="hljs-string">&quot;a &amp; b&quot;</span>',
      "&lt;div&gt;",
    ]);
  });

  it("drops a span that opens and closes without emitting any text", () => {
    expect(splitHighlightedLines('<span class="a"></span>x')).toEqual(["x"]);
  });

  it("tolerates an unbalanced closing tag rather than throwing", () => {
    // Defensive: hljs output is well-formed, but this runs on every rendered
    // block and a stray tag must degrade to plain text, not blank the drawer.
    expect(() => splitHighlightedLines("</span>oops")).not.toThrow();
    expect(splitHighlightedLines("</span>oops")).toEqual(["oops"]);
  });
});

/**
 * The cases above pin output for two grammars that were inspected by hand. These
 * drive real highlight.js instead, so a grammar nobody thought to check cannot
 * silently produce a shape the splitter mishandles. The invariants — not the
 * exact markup — are what matter, so they survive an hljs upgrade retokenizing
 * something differently.
 */
describe("splitHighlightedLines against real highlight.js output", () => {
  hljs.registerLanguage("python", python);
  hljs.registerLanguage("yaml", yaml);
  hljs.registerLanguage("typescript", typescript);
  hljs.registerLanguage("xml", xml);

  const SAMPLES: { language: string; code: string }[] = [
    {
      language: "python",
      code: 'def f():\n    """Doc\n    spans lines\n    """\n    return 1',
    },
    { language: "yaml", code: "key: |\n  line one\n  line two\nother: 1" },
    {
      // Template literal with an interpolation — nested spans across a newline.
      language: "typescript",
      code: "const a = `one ${\n  b\n} two`;\n/* block\n   comment */\nexport {};",
    },
    { language: "xml", code: '<!-- a\n     multiline comment -->\n<p class="x">hi</p>' },
    { language: "python", code: "" },
    { language: "python", code: "\n\n" },
  ];

  const strip = (s: string) => s.replace(/<span\b[^>]*>|<\/span>/g, "");

  for (const { language, code } of SAMPLES) {
    describe(`${language}: ${JSON.stringify(code.slice(0, 24))}`, () => {
      const lines = splitHighlightedLines(hljs.highlight(code, { language }).value);

      it("emits exactly one entry per source line", () => {
        expect(lines).toHaveLength(code.split("\n").length);
      });

      it("emits independently balanced markup on every line", () => {
        for (const line of lines) expect(isBalanced(line)).toBe(true);
      });

      it("preserves the highlighted text content", () => {
        // Compare against hljs' own escaping of the source rather than the raw
        // source: `<`, `&` and quotes are entities on both sides this way.
        expect(lines.map(strip).join("\n")).toBe(
          strip(hljs.highlight(code, { language }).value),
        );
      });
    });
  }
});
