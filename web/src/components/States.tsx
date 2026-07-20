import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { Button } from "./ui/Button";

export function ErrorState({
  error,
  onRetry,
}: {
  error: unknown;
  onRetry?: () => void;
}) {
  const status = error instanceof ApiError ? error.status : undefined;
  const message =
    error instanceof Error ? error.message : "Something went wrong.";
  return (
    <div className="rounded-lg border border-fail/30 bg-fail/5 p-6 text-sm">
      <div className="font-semibold text-fail">
        {status ? `Request failed (${status})` : "Request failed"}
      </div>
      <p className="mt-1 text-muted">{message}</p>
      {onRetry ? (
        <Button className="mt-3" size="sm" onClick={onRetry}>
          Retry
        </Button>
      ) : null}
    </div>
  );
}

export function EmptyState({
  title,
  children,
}: {
  title: string;
  children?: ReactNode;
}) {
  return (
    <div className="rounded-lg border border-dashed border-border p-10 text-center">
      <div className="text-sm font-medium text-fg">{title}</div>
      {children ? <div className="mt-1 text-sm text-muted">{children}</div> : null}
    </div>
  );
}
