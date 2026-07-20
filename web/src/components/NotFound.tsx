import { Link } from "react-router";

export function NotFound() {
  return (
    <div className="mx-auto max-w-md py-20 text-center">
      <div className="text-4xl font-semibold">404</div>
      <p className="mt-2 text-muted">This page does not exist.</p>
      <Link to="/" className="mt-4 inline-block text-accent hover:underline">
        Back to runs
      </Link>
    </div>
  );
}
