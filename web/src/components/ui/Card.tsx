import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * The app's surface container: `rounded-xl border border-border bg-surface`.
 *
 * Repeated verbatim at 20+ call sites before this existed. Adopted incrementally
 * — only in files already being changed — so the diff stays reviewable.
 *
 * `padding="flush"` is for containers that own their own insets: tables and
 * virtualized grids whose header/rows carry the horizontal padding, and which
 * need the card's rounded corners to clip them.
 */
export function Card({
  as: Tag = "div",
  padding = "md",
  className,
  children,
}: {
  as?: "div" | "section";
  padding?: "md" | "flush";
  className?: string;
  children: ReactNode;
}) {
  return (
    <Tag
      className={cn(
        "rounded-xl border border-border bg-surface",
        padding === "md" && "p-4",
        padding === "flush" && "overflow-hidden",
        className,
      )}
    >
      {children}
    </Tag>
  );
}
