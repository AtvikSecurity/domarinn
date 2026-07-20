import type { ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { Button } from "./Button";

/**
 * Keyboard-accessible modal built on Radix Dialog (focus trap, Esc to close,
 * scroll lock) — the same primitive as the case drawer + token modal.
 */
export function Modal({
  open,
  onOpenChange,
  title,
  description,
  children,
  size = "md",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  children: ReactNode;
  size?: "md" | "lg";
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] data-[state=open]:animate-[overlay-in_120ms_ease-out]" />
        <Dialog.Content
          className={
            "fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2 rounded-xl border border-border bg-surface p-5 shadow-2xl focus:outline-none " +
            (size === "lg"
              ? "w-[min(38rem,calc(100vw-2rem))]"
              : "w-[min(30rem,calc(100vw-2rem))]")
          }
        >
          <Dialog.Title className="text-base font-semibold">{title}</Dialog.Title>
          {description ? (
            <Dialog.Description className="mt-1 text-sm text-muted">
              {description}
            </Dialog.Description>
          ) : (
            <Dialog.Description className="sr-only">{title}</Dialog.Description>
          )}
          <div className="mt-4">{children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** A confirm/cancel footer commonly used inside `Modal`. */
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
