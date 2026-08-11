import { forwardRef } from "react";
import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/cn";
import {
  INTERACTIVE_OUTLINE_TONE,
  OUTLINE_LABEL_BASE,
  type OutlineTone,
} from "./chrome";

export interface PillButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  tone?: OutlineTone;
  size?: "xs" | "sm";
  pressed?: boolean;
}

export const PillButton = forwardRef<HTMLButtonElement, PillButtonProps>(
  (
    {
      tone = "neutral",
      size = "sm",
      pressed,
      type,
      className,
      "aria-pressed": ariaPressed,
      ...props
    },
    ref,
  ) => (
    <button
      ref={ref}
      type={type ?? "button"}
      aria-pressed={pressed ?? ariaPressed}
      data-pressed={pressed === undefined ? undefined : String(pressed)}
      className={cn(
        OUTLINE_LABEL_BASE,
        "select-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-bg disabled:cursor-not-allowed disabled:opacity-50",
        size === "xs"
          ? "px-1 py-0.5 text-[10px]"
          : "px-[7px] py-[3px] text-[11px]",
        INTERACTIVE_OUTLINE_TONE[tone],
        className,
      )}
      {...props}
    />
  ),
);
PillButton.displayName = "PillButton";
