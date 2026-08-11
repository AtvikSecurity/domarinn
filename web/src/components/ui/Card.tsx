import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { CHROME_FRAME } from "./chrome";

/**
 * The app's transparent chrome frame. The shared recipe owns only its outline;
 * callers continue to own padding, clipping, and layout.
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
        CHROME_FRAME,
        padding === "md" && "p-4",
        padding === "flush" && "overflow-hidden",
        className,
      )}
    >
      {children}
    </Tag>
  );
}
