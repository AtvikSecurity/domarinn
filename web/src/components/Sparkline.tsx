import { useId } from "react";
import { cn } from "@/lib/cn";

interface SparklineProps {
  /** Series values; drawn oldest -> newest, left -> right. */
  values: number[];
  width?: number;
  height?: number;
  /** Fixed value range; defaults to the data's min/max with light padding. */
  min?: number;
  max?: number;
  className?: string;
  title?: string;
}

/**
 * Dependency-free sparkline. Renders a smooth-ish polyline with a faint area
 * fill and a dot on the latest point. Colors come from CSS tokens so it tracks
 * light/dark automatically.
 */
export function Sparkline({
  values,
  width = 96,
  height = 26,
  min,
  max,
  className,
  title,
}: SparklineProps) {
  const gradId = useId();
  if (values.length === 0) {
    return (
      <svg width={width} height={height} className={cn("text-muted", className)} aria-hidden />
    );
  }

  const lo = min ?? Math.min(...values);
  const hiRaw = max ?? Math.max(...values);
  const hi = hiRaw === lo ? lo + 1 : hiRaw;
  const pad = 2;
  const w = width - pad * 2;
  const h = height - pad * 2;
  const stepX = values.length > 1 ? w / (values.length - 1) : 0;

  const points = values.map((v, i) => {
    const x = pad + i * stepX;
    const y = pad + h - ((v - lo) / (hi - lo)) * h;
    return [x, y] as const;
  });

  const line = points.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x.toFixed(1)} ${y.toFixed(1)}`).join(" ");

  const first = points[0];
  const last = points[points.length - 1];
  const firstVal = values[0];
  const lastVal = values[values.length - 1];
  // `values` is non-empty (guarded at the top) and `points` is 1:1 with it, so
  // both ends always exist; this guard is unreachable but narrows the indexed
  // access for noUncheckedIndexedAccess without a non-null assertion.
  if (!first || !last || firstVal === undefined || lastVal === undefined) {
    return (
      <svg width={width} height={height} className={cn("text-muted", className)} aria-hidden />
    );
  }

  const area = `${line} L${last[0].toFixed(1)} ${height - pad} L${first[0].toFixed(1)} ${height - pad} Z`;
  const trendUp = lastVal >= firstVal;
  const stroke = trendUp ? "var(--color-pass)" : "var(--color-fail)";

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      className={className}
      role="img"
      aria-label={title ?? "trend sparkline"}
    >
      {title ? <title>{title}</title> : null}
      <defs>
        <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={stroke} stopOpacity="0.22" />
          <stop offset="100%" stopColor={stroke} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#${gradId})`} stroke="none" />
      <path
        d={line}
        fill="none"
        stroke={stroke}
        strokeWidth={1.5}
        strokeLinejoin="round"
        strokeLinecap="round"
      />
      <circle cx={last[0]} cy={last[1]} r={2} fill={stroke} />
    </svg>
  );
}
