import { useRef, type ReactNode } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { cn } from "@/lib/cn";
import { Button } from "./Button";

const CloseIcon = () => (
  <svg
    width="16"
    height="16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    aria-hidden
  >
    <path d="M6 6l12 12M18 6L6 18" />
  </svg>
);

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
 *
 * A visible close button sits in the top-right corner, so dismissing does not
 * depend on knowing Esc or on finding a backdrop to click. It is rendered last
 * in the DOM (positioned absolutely) on purpose: Radix moves initial focus to
 * the first focusable element, and putting the close button first would steal
 * that from a form's first field or the dialog's primary action. Tab order is
 * therefore body, footer, then Close.
 *
 * Focus returns to whatever opened the dialog. Radix only restores focus to a
 * `Dialog.Trigger`, and every caller here opens the modal from state with no
 * trigger, so without this focus fell to `<body>` on close and a keyboard user
 * was dumped at the top of the page.
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
  const returnFocusRef = useRef<HTMLElement | null>(null);
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
          onOpenAutoFocus={() => {
            // Runs before Radix moves focus in, so this is still the opener.
            returnFocusRef.current =
              document.activeElement instanceof HTMLElement
                ? document.activeElement
                : null;
          }}
          onCloseAutoFocus={(e) => {
            const target = returnFocusRef.current;
            returnFocusRef.current = null;
            // The opener may be gone (e.g. a row deleted by this very dialog);
            // then fall back to Radix's default.
            if (target?.isConnected && target !== document.body) {
              e.preventDefault();
              target.focus();
            }
          }}
        >
          <Dialog.Title className="shrink-0 pr-8 text-base font-semibold">
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
          <Dialog.Close
            aria-label="Close"
            className="absolute right-3 top-3 grid size-8 place-items-center rounded-md text-muted hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <CloseIcon />
          </Dialog.Close>
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
