import type { ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { cn } from "@/lib/cn";
import { Button } from "./Button";

/**
 * Keyboard-accessible modal built on Radix Dialog (focus trap, Esc to close,
 * scroll lock).
 *
 * The content is a bounded flex column: header and `footer` stay put while the
 * body scrolls. Without that bound, a vertically-centred dialog taller than the
 * viewport had its top and bottom clipped off-screen with no scrollbar and no
 * way to reach them — Radix locks page scroll while open, so there was no outer
 * scroller to fall back on. Three stacked provider panels, or an unbounded diff,
 * hit that easily.
 */
export function Modal({
  open,
  onOpenChange,
  title,
  description,
  children,
  footer,
  size = "md",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  /** Optional: a confirm-only dialog is just a title, description and footer. */
  children?: ReactNode;
  /** Pinned below the scrolling body — actions stay reachable at any height. */
  footer?: ReactNode;
  size?: "md" | "lg" | "xl";
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] data-[state=open]:animate-[overlay-in_120ms_ease-out]" />
        <Dialog.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100dvh-4rem)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-xl border border-border bg-surface p-5 shadow-2xl focus:outline-none",
            size === "xl" && "w-[min(72rem,calc(100vw-2rem))]",
            size === "lg" && "w-[min(38rem,calc(100vw-2rem))]",
            size === "md" && "w-[min(30rem,calc(100vw-2rem))]",
          )}
        >
          <Dialog.Title className="shrink-0 text-base font-semibold">
            {title}
          </Dialog.Title>
          {description ? (
            <Dialog.Description className="mt-1 shrink-0 text-sm text-muted">
              {description}
            </Dialog.Description>
          ) : (
            <Dialog.Description className="sr-only">{title}</Dialog.Description>
          )}
          {children ? (
            <div className="mt-4 min-h-0 flex-1 overflow-y-auto overscroll-contain">
              {children}
            </div>
          ) : null}
          {footer ? <div className="shrink-0">{footer}</div> : null}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * A confirm/cancel footer for `Modal`. Pass it via the `footer` prop so it stays
 * pinned when the body scrolls.
 */
export function ModalActions({
  onCancel,
  onConfirm,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  confirmVariant = "primary",
  pending = false,
  confirmType = "button",
}: {
  onCancel: () => void;
  onConfirm?: () => void;
  confirmLabel?: string;
  cancelLabel?: string;
  confirmVariant?: "primary" | "danger";
  pending?: boolean;
  confirmType?: "button" | "submit";
}) {
  return (
    <div className="mt-5 flex justify-end gap-2">
      <Button type="button" variant="ghost" onClick={onCancel} disabled={pending}>
        {cancelLabel}
      </Button>
      <Button
        type={confirmType}
        variant={confirmVariant}
        onClick={onConfirm}
        disabled={pending}
      >
        {pending ? "Working…" : confirmLabel}
      </Button>
    </div>
  );
}
