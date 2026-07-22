import type { ReactNode } from "react";
import { Link, Navigate, Outlet, useLocation } from "react-router";
import { useAuth } from "./AuthProvider";
import { Button } from "@/components/ui/Button";
import { CenteredSpinner } from "@/components/ui/Spinner";

/** A friendly 403 panel for authenticated-but-unauthorized users. */
export function ForbiddenState({
  message = "You do not have permission to view this page.",
}: {
  message?: string;
}) {
  return (
    <div className="mx-auto max-w-md py-20 text-center">
      <div className="text-4xl font-semibold">403</div>
      <p className="mt-2 text-muted">{message}</p>
      <Link to="/" className="mt-4 inline-block">
        <Button variant="secondary" size="sm">
          Back to runs
        </Button>
      </Link>
    </div>
  );
}

/**
 * Gate every protected route behind a login. Used as a pathless layout route,
 * so it renders `<Outlet />` for its children when access is allowed.
 *
 * `isLoading` (meta + me both resolving) shows a spinner so the login page
 * never flashes on boot; once data exists, refetches don't re-trip it. If
 * meta itself errors, `needsLogin` stays false and children render their own
 * error states rather than trapping the user on `/login`.
 */
export function RequireAuth({ children }: { children?: ReactNode }) {
  const { view, isLoading } = useAuth();
  const location = useLocation();

  if (isLoading) return <CenteredSpinner label="Checking access…" />;
  if (view.needsLogin) {
    return <Navigate to="/login" state={{ from: location }} replace />;
  }
  return <>{children ?? <Outlet />}</>;
}

/** Gate a route on admin scope: redirect to login if anonymous, else 403. */
export function RequireAdmin({ children }: { children: ReactNode }) {
  const { view, isLoading } = useAuth();
  const location = useLocation();

  if (isLoading) return <CenteredSpinner label="Checking access…" />;
  if (view.promptLogin) {
    return <Navigate to="/login" state={{ from: location }} replace />;
  }
  if (!view.canAdmin) {
    return <ForbiddenState message="Admin access is required for this page." />;
  }
  return <>{children}</>;
}
