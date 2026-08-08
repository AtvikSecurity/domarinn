import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import {
  OUTLINE_LABEL_BASE,
  OUTLINE_LABEL_TONE,
  type OutlineTone,
} from "./chrome";

/**
 * A small static outline label: tags, `cached`, `baseline`, attempt counts,
 * stop reasons, chat/auth roles, and status metadata.
 */
export type ChipTone = OutlineTone;

export function Chip({
  children,
  tone = "neutral",
  size = "sm",
  mono: _mono,
  title,
  className,
  ...rest
}: {
  children: ReactNode;
  tone?: ChipTone;
  /** `xs` is for dense grid cells; `sm` is the default inline size. */
  size?: "xs" | "sm";
  /**
   * Accepted and ignored: the outline recipe is always monospace. Kept only so
   * call sites still carrying the old flag compile; delete the prop once they
   * have dropped it.
   *
   * @deprecated
   */
  mono?: boolean;
  title?: string;
  className?: string;
} & Omit<React.HTMLAttributes<HTMLSpanElement>, "className" | "title">) {
  return (
    <span
      title={title}
      className={cn(
        OUTLINE_LABEL_BASE,
        size === "xs"
          ? "px-1 py-0.5 text-[10px]"
          : "px-[7px] py-[3px] text-[11px]",
        OUTLINE_LABEL_TONE[tone],
        className,
      )}
      {...rest}
    >
      {children}
    </span>
  );
}
