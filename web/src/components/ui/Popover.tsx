import type { ReactNode } from "react";
import * as RPopover from "@radix-ui/react-popover";
import { cn } from "@/lib/cn";

export function Popover({
  trigger,
  children,
  align = "start",
  className,
  open,
  onOpenChange,
}: {
  trigger: ReactNode;
  children: ReactNode;
  align?: "start" | "center" | "end";
  className?: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  return (
    <RPopover.Root open={open} onOpenChange={onOpenChange}>
      <RPopover.Trigger asChild>{trigger}</RPopover.Trigger>
      <RPopover.Portal>
        <RPopover.Content
          align={align}
          sideOffset={6}
          className={cn(
            "z-50 rounded-lg border border-border bg-surface p-1 shadow-lg",
            "data-[state=open]:animate-[overlay-in_120ms_ease-out]",
            "focus:outline-none",
            className,
          )}
        >
          {children}
        </RPopover.Content>
      </RPopover.Portal>
    </RPopover.Root>
  );
}
