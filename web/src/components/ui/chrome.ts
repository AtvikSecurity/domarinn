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
  "inline-flex items-center gap-1 rounded-[8px] border bg-transparent font-mono font-medium uppercase tracking-[0.08em] leading-none transition-colors";

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
