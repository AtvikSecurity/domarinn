import { Fragment } from "react";
import { Link } from "react-router";
import { cn } from "@/lib/cn";

export interface BreadcrumbItem {
  label: string;
  /**
   * Where the crumb goes. Ancestors carry one; the trailing crumb does not —
   * it is where the reader already is.
   */
  to?: string;
}

/**
 * The trail above a drill-down page: `Sets / checkout-agent / regression`.
 *
 * The last item is always the current page — rendered as text with
 * `aria-current="page"`, never as a link, whatever `to` it was given. A
 * breadcrumb whose final crumb links to the page you are on announces as
 * somewhere else to go, and the separators are decoration, so they are hidden
 * from assistive tech rather than read out as part of a path.
 */
export function Breadcrumb({
  items,
  className,
}: {
  items: BreadcrumbItem[];
  className?: string;
}) {
  return (
    <nav
      aria-label="Breadcrumb"
      className={cn("flex flex-wrap items-center gap-2 text-sm", className)}
    >
      {items.map((item, i) => {
        const last = i === items.length - 1;
        return (
          <Fragment key={`${item.label}-${i}`}>
            {i > 0 ? (
              <span className="text-muted" aria-hidden="true">
                /
              </span>
            ) : null}
            {item.to && !last ? (
              <Link
                to={item.to}
                className="rounded-sm text-muted hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {item.label}
              </Link>
            ) : (
              <span
                className={last ? "text-fg" : "text-muted"}
                aria-current={last ? "page" : undefined}
              >
                {item.label}
              </span>
            )}
          </Fragment>
        );
      })}
    </nav>
  );
}
