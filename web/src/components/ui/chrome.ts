export const CHROME_FRAME =
  "rounded-lg border border-chrome-border bg-transparent shadow-[inset_0_1px_0_0_var(--color-chrome-highlight)]";

export type OutlineTone =
  | "neutral"
  | "info"
  | "accent"
  | "pass"
  | "fail"
  | "error"
  | "amber"
  | "skip";

export const OUTLINE_LABEL_BASE =
  "inline-flex items-center gap-1 rounded-[8px] border bg-transparent font-medium leading-none transition-colors";

export const OUTLINE_LABEL_TONE: Record<OutlineTone, string> = {
  neutral: "border-border-strong text-muted",
  info: "border-info text-info",
  accent: "border-accent text-accent",
  pass: "border-pass text-pass",
  fail: "border-fail text-fail",
  error: "border-error text-error",
  amber: "border-amber text-amber",
  skip: "border-skip text-skip",
};

/**
 * Tab strip item, per the Atvik design system's tab treatment: no container
 * chrome at all, with the selection carried by a rule under the active label.
 *
 * The rule is a transparent border present on *every* item rather than one
 * added to the selected item. Colouring a border that is already there keeps
 * all the labels on a common baseline; adding one only to the active item
 * shunts it 2px down as the selection moves.
 *
 * Size stays with the caller — a page-level filter strip and a dense toolbar
 * toggle want different padding — so only the frame and the two states live
 * here. Guarded by chrome.drift.test.ts so the recipe keeps one home.
 */
export const TAB_ITEM_BASE =
  "border-b-2 border-transparent font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";
export const TAB_ITEM_SELECTED = "border-info text-fg";
export const TAB_ITEM_IDLE = "text-muted hover:border-border-strong hover:text-fg";
/** Disabled sits on top of `TAB_ITEM_IDLE`, so it has to undo its hover. */
export const TAB_ITEM_DISABLED =
  "cursor-not-allowed opacity-40 hover:border-transparent hover:text-muted";

export const INTERACTIVE_OUTLINE_TONE: Record<OutlineTone, string> = {
  neutral: "border-border-strong text-muted hover:bg-fg/5 data-[pressed=true]:bg-fg/5",
  info: "border-info text-info hover:bg-info/8 data-[pressed=true]:bg-info/8",
  accent: "border-accent text-accent hover:bg-accent/8 data-[pressed=true]:bg-accent/8",
  pass: "border-pass text-pass hover:bg-pass/8 data-[pressed=true]:bg-pass/8",
  fail: "border-fail text-fail hover:bg-fail/8 data-[pressed=true]:bg-fail/8",
  error: "border-error text-error hover:bg-error/8 data-[pressed=true]:bg-error/8",
  amber: "border-amber text-amber hover:bg-amber/8 data-[pressed=true]:bg-amber/8",
  skip: "border-skip text-skip hover:bg-skip/8 data-[pressed=true]:bg-skip/8",
};
