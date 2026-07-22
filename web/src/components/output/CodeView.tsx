import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import { cn } from "@/lib/cn";

// Only the languages we register are ever highlighted — keeps the lazy chunk
// small and makes `highlightAuto` deterministic over a known set. The `.hljs-*`
// theme lives in index.css and maps onto the design tokens (no shipped theme).
const LANGUAGES: Record<string, LanguageFn> = {
  json,
  xml,
  javascript,
  typescript,
  python,
  bash,
  yaml,
  markdown,
  sql,
  rust,
};

type LanguageFn = Parameters<typeof hljs.registerLanguage>[1];

for (const [name, fn] of Object.entries(LANGUAGES)) {
  hljs.registerLanguage(name, fn);
}

const SUPPORTED = Object.keys(LANGUAGES);

// Common shorthands map onto the registered language names.
const ALIASES: Record<string, string> = {
  js: "javascript",
  jsx: "javascript",
  ts: "typescript",
  tsx: "typescript",
  py: "python",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  yml: "yaml",
  rs: "rust",
  html: "xml",
  md: "markdown",
};

function resolveLanguage(hint?: string): string | undefined {
  if (!hint) return undefined;
  const key = hint.toLowerCase();
  const resolved = ALIASES[key] ?? key;
  return SUPPORTED.includes(resolved) ? resolved : undefined;
}

function highlight(code: string, hint?: string): string {
  const language = resolveLanguage(hint);
  try {
    if (language) return hljs.highlight(code, { language }).value;
    return hljs.highlightAuto(code, SUPPORTED).value;
  } catch {
    // hljs already HTML-escapes its output; on the error path escape ourselves.
    return code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }
}

/**
 * Syntax-highlighted code block. Lazy-loaded (see the barrel) so highlight.js
 * never lands in the main bundle. The highlighted markup comes from
 * highlight.js, which escapes its input, so `dangerouslySetInnerHTML` is safe.
 */
export default function CodeView({
  code,
  langHint,
  wrap = false,
  maxHeight,
  className,
}: {
  code: string;
  langHint?: string;
  wrap?: boolean;
  maxHeight?: string;
  className?: string;
}) {
  const html = highlight(code, langHint);
  return (
    <pre
      style={maxHeight ? { maxHeight } : undefined}
      className={cn(
        "overflow-auto rounded-lg border border-border bg-bg p-3 font-mono text-xs leading-relaxed",
        wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre",
        className,
      )}
    >
      <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
}
