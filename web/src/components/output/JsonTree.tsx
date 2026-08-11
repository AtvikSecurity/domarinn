import { useState } from "react";
import { cn } from "@/lib/cn";
import { CodeSurface } from "./CodeSurface";
import { outputToString } from "./detect";

/**
 * A bespoke, dependency-free collapsible JSON viewer. Objects and arrays are
 * expandable; nodes with more than {@link COLLAPSE_THRESHOLD} children start
 * collapsed. Expand-all / collapse-all is broadcast to every node through a
 * generation counter (bumping it re-syncs each node's open state during render,
 * never in an effect). Long strings truncate with an inline expander.
 *
 * The tree sits inside the shared {@link CodeSurface}, so a payload here reads
 * like one anywhere else in the app and picks up copy and soft wrap. The tree
 * body itself is unchanged: collapsing beats a flat code block for finding your
 * way around a deep object, which is why this is not simply a highlighted dump.
 */

const COLLAPSE_THRESHOLD = 20;
const STRING_LIMIT = 140;

type Entry = [key: string, value: unknown];

function entriesOf(value: unknown): { entries: Entry[]; isArray: boolean } {
  if (Array.isArray(value)) {
    return { entries: value.map((v, i) => [String(i), v]), isArray: true };
  }
  const rec = value as Record<string, unknown>;
  return { entries: Object.entries(rec), isArray: false };
}

export function JsonTree({
  data,
  wrap: wrapProp,
  onWrapChange,
  defaultWrap = true,
  maxHeight,
  className,
}: {
  data: unknown;
  /** Controlled soft wrap. Pair with `onWrapChange`; omit both to own the state. */
  wrap?: boolean;
  onWrapChange?: (wrap: boolean) => void;
  defaultWrap?: boolean;
  maxHeight?: string;
  className?: string;
}) {
  // `gen` increments on expand/collapse-all; `genOpen` is the state that change
  // forces onto every node.
  const [gen, setGen] = useState(0);
  const [genOpen, setGenOpen] = useState(true);
  const [localWrap, setLocalWrap] = useState(defaultWrap);
  const wrap = wrapProp ?? localWrap;

  const isContainer = data !== null && typeof data === "object";

  return (
    <CodeSurface
      testId="json-tree"
      label="json"
      copyValue={outputToString(data)}
      wrap={wrap}
      onWrapChange={(next) => {
        if (onWrapChange) onWrapChange(next);
        else setLocalWrap(next);
      }}
      maxHeight={maxHeight}
      className={className}
      // Soft wrap is applied once, here, and reaches every node by inheritance —
      // `white-space`, `word-break` and `overflow-wrap` are all inherited
      // properties. The alternative was threading a `wrap` prop down a recursive
      // component that already drills two.
      bodyClassName={cn(
        "px-3",
        wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre",
      )}
      actions={
        isContainer ? (
          <div className="flex items-center gap-1">
            <TreeButton
              onClick={() => {
                setGenOpen(true);
                setGen((g) => g + 1);
              }}
            >
              Expand all
            </TreeButton>
            <TreeButton
              onClick={() => {
                setGenOpen(false);
                setGen((g) => g + 1);
              }}
            >
              Collapse all
            </TreeButton>
          </div>
        ) : null
      }
    >
      <Node value={data} depth={0} isRoot gen={gen} genOpen={genOpen} />
    </CodeSurface>
  );
}

function TreeButton({
  onClick,
  children,
}: {
  onClick: () => void;
  children: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="rounded px-1.5 py-0.5 text-[11px] font-medium text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      {children}
    </button>
  );
}

interface NodeProps {
  /** Object key or array index this node hangs off; absent for the root. */
  label?: string;
  /** True when `label` is an object key (rendered quoted), false for indices. */
  quotedKey?: boolean;
  value: unknown;
  depth: number;
  isRoot?: boolean;
  gen: number;
  genOpen: boolean;
}

function Node({ label, quotedKey, value, depth, isRoot, gen, genOpen }: NodeProps) {
  const isContainer = value !== null && typeof value === "object";
  const { entries, isArray } = isContainer
    ? entriesOf(value)
    : { entries: [] as Entry[], isArray: false };
  const childCount = entries.length;

  const initialOpen = isRoot || childCount <= COLLAPSE_THRESHOLD;
  const [open, setOpen] = useState(initialOpen);
  // Re-sync to the broadcast open state when the generation changes (the
  // adjust-state-during-render pattern this repo uses instead of an effect).
  const [seenGen, setSeenGen] = useState(gen);
  if (gen !== seenGen) {
    setSeenGen(gen);
    setOpen(isRoot ? true : genOpen);
  }

  const keyEl = label !== undefined ? <Key label={label} quoted={quotedKey} /> : null;

  if (!isContainer) {
    // No wrap classes here: the surface body sets them once and they inherit,
    // so the whole tree switches together when the toggle flips.
    return (
      <div>
        {keyEl}
        <Primitive value={value} />
      </div>
    );
  }

  const openBrace = isArray ? "[" : "{";
  const closeBrace = isArray ? "]" : "}";
  const summary = `${childCount} ${isArray ? (childCount === 1 ? "item" : "items") : childCount === 1 ? "key" : "keys"}`;

  return (
    <div>
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setOpen((o) => !o);
          }
        }}
        className="flex cursor-pointer select-none items-center gap-1 rounded outline-none hover:bg-surface-2/60 focus-visible:ring-2 focus-visible:ring-ring"
      >
        <Chevron open={open} />
        {keyEl}
        <span className="text-muted">{openBrace}</span>
        {!open ? (
          <>
            <span className="px-1 text-muted/70">{summary}</span>
            <span className="text-muted">{closeBrace}</span>
          </>
        ) : null}
      </div>
      {open ? (
        <div className="ml-2 border-l border-border pl-3">
          {entries.map(([k, v]) => (
            <Node
              key={k}
              label={k}
              quotedKey={!isArray}
              value={v}
              depth={depth + 1}
              gen={gen}
              genOpen={genOpen}
            />
          ))}
          <div className="text-muted">{closeBrace}</div>
        </div>
      ) : null}
    </div>
  );
}

function Key({ label, quoted }: { label: string; quoted?: boolean }) {
  return (
    <span className="text-accent">
      {quoted ? `"${label}"` : label}
      <span className="text-muted">: </span>
    </span>
  );
}

function Chevron({ open }: { open: boolean }) {
  return (
    <svg
      width="10"
      height="10"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      className={cn("shrink-0 text-muted transition-transform", open && "rotate-90")}
    >
      <path d="M9 6l6 6-6 6" />
    </svg>
  );
}

function Primitive({ value }: { value: unknown }) {
  if (value === null) return <span className="text-muted">null</span>;
  switch (typeof value) {
    case "string":
      return <StringValue text={value} />;
    case "number":
    case "bigint":
      return <span className="text-amber tabular-nums">{String(value)}</span>;
    case "boolean":
      return <span className="text-error">{String(value)}</span>;
    default:
      // Objects go through the container path; functions/symbols/undefined
      // never appear in parsed JSON, so there is nothing to render here.
      return null;
  }
}

function StringValue({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);
  const long = text.length > STRING_LIMIT;
  const shown = long && !expanded ? `${text.slice(0, STRING_LIMIT)}…` : text;
  return (
    <span className="text-pass">
      {`"${shown}"`}
      {long ? (
        <button
          type="button"
          onClick={() => setExpanded((e) => !e)}
          className="ml-1 rounded px-1 text-[11px] font-medium text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          {expanded ? "less" : "more"}
        </button>
      ) : null}
    </span>
  );
}
