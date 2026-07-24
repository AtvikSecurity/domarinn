import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * A small inline label: tags, `cached`, `baseline`, attempt counts, stop reasons,
 * chat/auth roles.
 *
 * One concept had three visual definitions before this — the `cached` chip alone
 * appeared in three sizes, three paddings and two font weights depending on
 * which file rendered it.
 *
 * Tone classes are written out literally rather than interpolated, because
 * Tailwind only sees class names it can find as complete strings in the source.
 */
export type ChipTone =
  | "neutral"
  | "accent"
  | "pass"
  | "fail"
  | "error"
  | "amber"
  | "skip";

const TONE: Record<ChipTone, string> = {
  neutral: "bg-surface-2 text-muted",
  accent: "bg-accent/12 text-accent",
  pass: "bg-pass/12 text-pass",
  fail: "bg-fail/12 text-fail",
  error: "bg-error/12 text-error",
  amber: "bg-amber/12 text-amber",
  skip: "bg-skip/12 text-skip",
};

export function Chip({
  children,
  tone = "neutral",
  size = "sm",
  mono = false,
  title,
  className,
  ...rest
}: {
  children: ReactNode;
  tone?: ChipTone;
  /** `xs` is for dense grid cells; `sm` is the default inline size. */
  size?: "xs" | "sm";
  mono?: boolean;
  title?: string;
  className?: string;
} & Omit<React.HTMLAttributes<HTMLSpanElement>, "className" | "title">) {
  return (
    <span
      title={title}
      className={cn(
        "rounded font-medium",
        size === "xs" ? "px-1 py-px text-[10px]" : "px-1.5 py-0.5 text-[11px]",
        mono && "font-mono",
        TONE[tone],
        className,
      )}
      {...rest}
    >
      {children}
    </span>
  );
}
