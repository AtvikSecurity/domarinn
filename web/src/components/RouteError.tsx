import { Link, useRouteError } from "react-router";

/**
 * Route-level error boundary (`errorElement`). Without one, any render error
 * replaces the entire app — navigation included — with react-router's
 * unstyled "Unexpected Application Error!" screen.
 */
export function RouteError() {
  const error = useRouteError();
  const message =
    error instanceof Error ? error.message : "An unexpected error occurred.";
  return (
    <div className="mx-auto max-w-md py-20 text-center">
      <div className="text-lg font-semibold">Something went wrong</div>
      <p className="mt-2 break-words font-mono text-sm text-muted">{message}</p>
      <Link to="/" className="mt-4 inline-block text-accent hover:underline">
        Back to overview
      </Link>
    </div>
  );
}
