import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";

async function writeClipboard(text: string): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    /* fall through to the legacy path */
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "absolute";
  ta.style.left = "-9999px";
  document.body.appendChild(ta);
  ta.select();
  try {
    document.execCommand("copy");
  } catch {
    /* best effort */
  }
  document.body.removeChild(ta);
}

/**
 * Small copy-to-clipboard control. `label` names it for assistive tech; when
 * `iconOnly` is false the label is also shown, flipping to "Copied" briefly.
 */
export function CopyButton({
  value,
  label = "Copy",
  iconOnly = false,
  className,
  tabIndex,
}: {
  value: string;
  label?: string;
  iconOnly?: boolean;
  className?: string;
  /**
   * Set to `-1` inside a virtualized row. A copy button per row would
   * otherwise double the grid's tab stops, and the row itself is already
   * focusable — the drawer's copy button is the keyboard path to the value.
   */
  tabIndex?: number;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => () => clearTimeout(timer.current), []);

  const onClick = useCallback(async () => {
    await writeClipboard(value);
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1200);
  }, [value]);

  return (
    <button
      type="button"
      onClick={onClick}
      tabIndex={tabIndex}
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-xs font-medium text-muted transition-colors hover:bg-surface-2 hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        className,
      )}
    >
      {copied ? (
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <path d="M20 6 9 17l-5-5" />
        </svg>
      ) : (
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <rect x="9" y="9" width="13" height="13" rx="2" />
          <path d="M5 15V5a2 2 0 0 1 2-2h10" />
        </svg>
      )}
      {iconOnly ? null : <span>{copied ? "Copied" : label}</span>}
    </button>
  );
}
