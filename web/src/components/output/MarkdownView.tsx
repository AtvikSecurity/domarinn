import type { ReactNode } from "react";
import Markdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { CodeBlock } from "./CodeBlock";
import { setWrap, useOutputPrefs } from "./prefs";

/** Flatten a code node's children to plain text without stringifying elements
 *  (react-markdown hands code content down as strings/arrays of strings). */
function nodeToText(node: ReactNode): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(nodeToText).join("");
  return "";
}

/**
 * Fenced (or any multiline) code delegates to the canonical block; a short
 * inline span stays a plain `<code>`.
 *
 * Soft wrap reads the shared preference rather than being pinned on, so a fence
 * inside a rendered document agrees with every other monospace surface on
 * screen — the drawer routinely shows both at once. It is a real component, not
 * an inline arrow in the map below, because it calls a hook.
 */
function CodeRenderer({
  className,
  children,
}: {
  className?: string;
  children?: ReactNode;
}) {
  const { wrap } = useOutputPrefs();
  const text = nodeToText(children).replace(/\n$/, "");
  const match = /language-(\w+)/.exec(className ?? "");
  if (match || text.includes("\n")) {
    return (
      <CodeBlock
        code={text}
        language={match?.[1]}
        wrap={wrap}
        onWrapChange={setWrap}
        className="my-2"
      />
    );
  }
  return (
    <code className="rounded bg-surface-2 px-1 py-0.5 font-mono text-[0.9em]">
      {children}
    </code>
  );
}

// Token-styled renderers for the subset of markdown providers actually emit.
// Raw HTML stays disabled (react-markdown's default — no rehype-raw), so
// untrusted output can't inject markup.
const COMPONENTS: Components = {
  h1: ({ children }) => (
    <h1 className="mb-2 mt-1 text-base font-semibold text-fg">{children}</h1>
  ),
  h2: ({ children }) => (
    <h2 className="mb-2 mt-3 text-sm font-semibold text-fg">{children}</h2>
  ),
  h3: ({ children }) => (
    <h3 className="mb-1.5 mt-3 text-sm font-semibold text-fg">{children}</h3>
  ),
  h4: ({ children }) => (
    <h4 className="mb-1.5 mt-2 text-xs font-semibold uppercase tracking-wide text-muted">
      {children}
    </h4>
  ),
  p: ({ children }) => <p className="my-2 leading-relaxed text-fg/90">{children}</p>,
  a: ({ href, children }) => (
    <a
      href={href}
      target="_blank"
      rel="noreferrer noopener"
      className="text-accent hover:underline"
    >
      {children}
    </a>
  ),
  ul: ({ children }) => (
    <ul className="my-2 ml-5 list-disc space-y-1 text-fg/90">{children}</ul>
  ),
  ol: ({ children }) => (
    <ol className="my-2 ml-5 list-decimal space-y-1 text-fg/90">{children}</ol>
  ),
  li: ({ children }) => <li className="leading-relaxed">{children}</li>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-border pl-3 text-muted italic">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-3 border-border" />,
  strong: ({ children }) => <strong className="font-semibold text-fg">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="w-full border-collapse text-xs">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead className="bg-surface-2">{children}</thead>,
  th: ({ children }) => (
    <th className="border border-border px-2 py-1 text-left font-semibold">
      {children}
    </th>
  ),
  td: ({ children }) => (
    <td className="border border-border px-2 py-1 align-top">{children}</td>
  ),
  code: CodeRenderer,
};

/**
 * Markdown renderer. Lazy-loaded (see the barrel) so react-markdown/remark-gfm
 * stay out of the main bundle.
 */
export default function MarkdownView({ markdown }: { markdown: string }) {
  return (
    <div className="rounded-lg border border-border bg-bg px-3 py-1 text-sm">
      <Markdown remarkPlugins={[remarkGfm]} components={COMPONENTS}>
        {markdown}
      </Markdown>
    </div>
  );
}
