import { forwardRef } from "react";
import type { ReactNode } from "react";
import { Chip } from "@/components/ui/Chip";
import { CopyButton } from "@/components/ui/CopyButton";
import { PillButton } from "@/components/ui/PillButton";
import { cn } from "@/lib/cn";

/**
 * The shared frame around every monospace payload: a header strip naming the
 * content and carrying copy / soft-wrap, over a body the caller fills.
 *
 * Three bodies mount inside it — the highlighted line grid (`CodeBlock`), the
 * collapsible JSON tree (`JsonTree`), and the raw text behind the Rendered/Raw
 * toggle — so a payload looks the same wherever it turns up and gains copy
 * without each viewer re-implementing a toolbar.
 *
 * Two layout rules here are load-bearing rather than cosmetic:
 *
 *   - **The type scale sits on the frame, not the body.** Font size inherits, and
 *     several callers pass `className="text-[11px]/relaxed"` to tighten a nested
 *     payload. Setting the size on the body would leave those overrides on an
 *     element that no longer controls it. (`lib/cn.test.ts` documents the related
 *     `text-*` / `leading-*` merge trap these primitives keep walking into.)
 *   - **`actions` stay visible; the tools hide until hover or focus.** Whatever a
 *     caller puts in `actions` is a primary affordance — the tree's expand and
 *     collapse are how you read it at all — while copy and wrap are recoverable
 *     and can stay out of the way. Revealing on `focus-within` as well as hover
 *     is what keeps them reachable from the keyboard.
 */
export interface CodeSurfaceProps {
  /** Header chip: the content type or language, e.g. `json`. */
  label: string;
  /** Exact text the copy button puts on the clipboard. */
  copyValue: string;
  /** Controlled soft wrap. Pair with `onWrapChange`; omit both to hide the toggle. */
  wrap?: boolean;
  onWrapChange?: (wrap: boolean) => void;
  /** Always-visible header controls, rendered next to the label. */
  actions?: ReactNode;
  /** Cap the body and give it a scrollbar. */
  maxHeight?: string;
  /** Base for this surface's test ids; the body gets `${testId}-body`. */
  testId?: string;
  className?: string;
  /** Extra classes for the body wrapper (the scroll container). */
  bodyClassName?: string;
  children: ReactNode;
}

export const CodeSurface = forwardRef<HTMLDivElement, CodeSurfaceProps>(
  (
    {
      label,
      copyValue,
      wrap,
      onWrapChange,
      actions,
      maxHeight,
      testId = "code-surface",
      className,
      bodyClassName,
      children,
    },
    ref,
  ) => (
    <div
      ref={ref}
      data-testid={testId}
      className={cn(
        "group/surface overflow-hidden rounded-lg border border-border bg-bg font-mono text-xs leading-relaxed",
        className,
      )}
    >
      <div className="flex items-center gap-2 border-b border-border bg-surface-2 px-2 py-1">
        <Chip size="xs">{label}</Chip>
        {actions}
        <div className="ml-auto flex items-center gap-1 opacity-0 transition-opacity group-focus-within/surface:opacity-100 group-hover/surface:opacity-100">
          {onWrapChange ? (
            <PillButton
              size="xs"
              pressed={wrap}
              onClick={() => onWrapChange(!wrap)}
              title="Toggle soft wrap"
              data-testid={`${testId}-wrap-toggle`}
            >
              Wrap
            </PillButton>
          ) : null}
          {/* Not `iconOnly`: this replaced the viewer's own copy button, and that
              one showed a "Copied" confirmation. A silent icon would be a quieter
              control than the one it stands in for. */}
          <CopyButton value={copyValue} label="Copy" />
        </div>
      </div>
      {/* Owns both scroll axes: vertical only when a cap was asked for,
          horizontal only when wrapping is off. */}
      <div
        data-testid={`${testId}-body`}
        style={maxHeight ? { maxHeight } : undefined}
        className={cn(
          "py-2",
          maxHeight ? "overflow-y-auto" : "",
          wrap === false ? "overflow-x-auto" : "",
          bodyClassName,
        )}
      >
        {children}
      </div>
    </div>
  ),
);
CodeSurface.displayName = "CodeSurface";
