import { forwardRef } from "react";
import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/cn";

type Variant = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md";

/**
 * Colour comes from the `.btn-*` recipe in index.css — a flat fill, a coloured
 * hairline and a matching label, per the Atvik design system. It lives in CSS
 * because each variant needs a different value in each theme; only the weight
 * differs here, since the spec sets the two filled variants a step heavier than
 * the two quiet ones.
 */
const VARIANT: Record<Variant, string> = {
  primary: "btn-primary font-semibold",
  secondary: "btn-outline",
  ghost: "btn-ghost",
  danger: "btn-danger font-semibold",
};

const SIZE: Record<Size, string> = {
  sm: "h-7 px-2.5 text-xs gap-1.5",
  md: "h-9 px-3.5 text-sm gap-2",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = "secondary", size = "md", className, ...props }, ref) => (
    <button
      ref={ref}
      className={cn(
        // `border` with no colour: every variant sets its own `border-color`,
        // and a width has to exist for that to paint. Ghost's is transparent,
        // so switching variants never shifts a button by a pixel.
        //
        // `transition` and not `transition-colors`: the recipe animates the
        // inset highlight and a half-pixel press, and the -colors shorthand
        // covers neither box-shadow nor transform, so both would jump.
        "inline-flex select-none items-center justify-center rounded-md border font-medium transition",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-bg",
        "disabled:cursor-not-allowed disabled:opacity-50",
        VARIANT[variant],
        SIZE[size],
        className,
      )}
      {...props}
    />
  ),
);
Button.displayName = "Button";
