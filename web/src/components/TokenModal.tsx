import { useEffect, useRef, useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { clearToken, getToken, onUnauthorized, setToken } from "@/lib/auth";
import { useAuth } from "@/auth/AuthProvider";
import { Button } from "./ui/Button";

/**
 * Global token modal. Opens automatically when a request 401s (via the
 * onUnauthorized bus) and can also be opened manually from Settings.
 *
 * In `closed` mode the modal stays out of the way: a 401 there means the
 * session cookie expired, and the right recovery is the login page (which
 * RequireAuth redirects to when AuthProvider refreshes `me`), not pasting a
 * bearer token. The token path only makes sense for open/protect-writes
 * static-token deployments.
 */
export function TokenModal() {
  const { view } = useAuth();
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState("");
  // Mirror the live auth mode into a ref so the stable onUnauthorized handler
  // (registered once) always sees the current value. Written from an effect,
  // not during render.
  const authModeRef = useRef(view.authMode);
  useEffect(() => {
    authModeRef.current = view.authMode;
  }, [view.authMode]);

  // Load the current token when the dialog opens (from either path) rather
  // than reacting to `open` in an effect, which would set state synchronously.
  function openWithCurrentToken() {
    setValue(getToken() ?? "");
    setOpen(true);
  }
  useEffect(
    () =>
      onUnauthorized(() => {
        if (authModeRef.current === "closed") return;
        openWithCurrentToken();
      }),
    [],
  );

  function handleOpenChange(next: boolean) {
    if (next) openWithCurrentToken();
    else setOpen(false);
  }

  function save() {
    const trimmed = value.trim();
    if (trimmed) setToken(trimmed);
    else clearToken();
    setOpen(false);
  }

  return (
    <Dialog.Root open={open} onOpenChange={handleOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] data-[state=open]:animate-[overlay-in_120ms_ease-out]" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded-xl border border-border bg-surface p-5 shadow-2xl focus:outline-none">
          <Dialog.Title className="text-base font-semibold">Access token required</Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-muted">
            This server needs a bearer token. It is stored locally in your browser
            and sent as <code className="font-mono text-xs">Authorization: Bearer …</code>.
          </Dialog.Description>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              save();
            }}
          >
            <input
              autoFocus
              type="password"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              placeholder="paste token"
              className="mt-4 w-full rounded-md border border-border bg-bg px-3 py-2 font-mono text-sm outline-none focus:ring-2 focus:ring-ring"
            />
            <div className="mt-4 flex justify-end gap-2">
              <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" variant="primary">
                Save token
              </Button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
