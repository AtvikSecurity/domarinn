import { describe, expect, it } from "vitest";
import { cn } from "./cn";

/**
 * `cn` is `twMerge(clsx(...))`, and tailwind-merge has no knowledge of this
 * project's custom `@theme` colour tokens (`surface-2`, `pass`, `fail`, `bg`,
 * …) — it classifies them by class *shape*. These tests pin the groupings the
 * codebase actually relies on, so a tailwind-merge upgrade that reclassifies a
 * token fails here rather than silently repainting the UI.
 */
describe("cn — conflict resolution", () => {
  it("resolves the conflict that was previously decided by stylesheet order", () => {
    // CaseGrid's selected row passes both; the later argument must win.
    expect(cn("hover:bg-surface-2", "hover:bg-accent/10")).toBe(
      "hover:bg-accent/10",
    );
  });

  it("treats the custom colour tokens as background colours, not positions", () => {
    // `bg-bg` is the token most at risk of being read as background-position.
    expect(cn("bg-bg/40", "bg-surface")).toBe("bg-surface");
    expect(cn("bg-pass/70", "bg-fail/70")).toBe("bg-fail/70");
    expect(cn("bg-transparent", "bg-surface")).toBe("bg-surface");
  });

  it("keeps last-wins for the other tokenised colour utilities", () => {
    expect(cn("border-pass/50", "border-fail/50")).toBe("border-fail/50");
    expect(cn("border-chrome-border", "border-fail/50")).toBe("border-fail/50");
    expect(cn("text-fg/90", "text-fail")).toBe("text-fail");
    expect(cn("ring-pass/25", "ring-fail/25")).toBe("ring-fail/25");
  });
});

describe("cn — groups that must NOT collapse", () => {
  it("keeps ring width and ring colour", () => {
    // The project's chip formula is `ring-1 ring-inset ring-<tone>/25`.
    expect(cn("ring-1 ring-inset", "ring-pass/25")).toBe(
      "ring-1 ring-inset ring-pass/25",
    );
  });

  it("keeps font size and text colour", () => {
    expect(cn("text-[10px]", "text-muted")).toBe("text-[10px] text-muted");
    expect(cn("text-xs", "text-fail")).toBe("text-xs text-fail");
  });

  it("keeps numeric-variant and colour utilities", () => {
    expect(cn("tabular-nums", "text-muted")).toBe("tabular-nums text-muted");
  });
});

describe("cn — the font-size / line-height trap", () => {
  // In Tailwind v4 every `text-<size>` also sets a line-height, so
  // tailwind-merge treats a later `text-*` size as overriding an earlier
  // `leading-*`. This bites the monospace primitives (RawText, JsonTree,
  // CodeBlock), whose base is `text-xs leading-relaxed` and which accept a
  // `className` size override from the caller.
  it("keeps leading-* when it follows the size", () => {
    expect(cn("text-[10px] leading-none")).toBe("text-[10px] leading-none");
    expect(cn("text-[11px]", "leading-none")).toBe("text-[11px] leading-none");
    expect(cn("text-xs", "leading-relaxed")).toBe("text-xs leading-relaxed");
  });

  it("drops leading-* when a size follows it — arbitrary or named", () => {
    expect(cn("leading-none", "text-[11px]")).toBe("text-[11px]");
    expect(cn("leading-relaxed", "text-xs")).toBe("text-xs");
  });

  it("preserves the base leading when the override uses the size/leading shorthand", () => {
    // This is why the drawer's dense blocks pass `text-[11px]/relaxed` rather
    // than a bare `text-[11px]` — see CaseDrawer / CaseDrawerSections.
    const base = "p-3 font-mono text-xs leading-relaxed";
    expect(cn(base, "mt-2 text-[11px]")).not.toContain("leading-relaxed");
    expect(cn(base, "mt-2 text-[11px]/relaxed")).toContain(
      "text-[11px]/relaxed",
    );
  });

  it("leaves leading-* alone next to a text COLOUR", () => {
    expect(cn("leading-relaxed", "text-fg/90")).toBe(
      "leading-relaxed text-fg/90",
    );
  });
});

describe("cn — ordinary Tailwind groups", () => {
  it("collapses sizing, spacing and radius", () => {
    expect(cn("size-3.5", "size-6")).toBe("size-6");
    expect(cn("px-1", "px-3")).toBe("px-3");
    expect(cn("rounded-[5px]", "rounded-full")).toBe("rounded-full");
    expect(cn("rounded-lg", "rounded-[3px]")).toBe("rounded-[3px]");
  });

  it("collapses inset utilities without touching position", () => {
    expect(cn("sticky top-0", "top-8")).toBe("sticky top-8");
  });

  it("still behaves like clsx for conditionals and falsy values", () => {
    const off = "" as string;
    expect(cn("a", off && "b", null, undefined, "c")).toBe("a c");
    expect(cn(["a", "b"], { c: true, d: false })).toBe("a b c");
  });
});
