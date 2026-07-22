import { Fragment } from "react";
import { SNIPPET_CLOSE, SNIPPET_OPEN } from "@/api/snippet";

/**
 * A search-result excerpt. The server (and the mock) delimit each matched
 * token with the PUA marker pair from `@/api/snippet`; every delimited span
 * renders as a highlighted `<mark>`.
 */
export function Snippet({ text, className }: { text: string; className?: string }) {
  const segments = text.split(SNIPPET_OPEN);
  return (
    <span className={className}>
      {segments.map((segment, i) => {
        if (i === 0) return <Fragment key={i}>{segment}</Fragment>;
        const [marked, ...rest] = segment.split(SNIPPET_CLOSE);
        return (
          <Fragment key={i}>
            <mark className="rounded-[2px] bg-amber/25 px-px text-fg">{marked}</mark>
            {rest.join(SNIPPET_CLOSE)}
          </Fragment>
        );
      })}
    </span>
  );
}
