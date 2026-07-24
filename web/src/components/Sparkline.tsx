import { useId } from "react";
import { cn } from "@/lib/cn";

interface SparklineProps {
  /**
   * Series values, oldest -> newest. `null` marks a gap: the slot is kept (so
   * x-positions stay aligned with whatever is rendered above the chart) and the
   * line is broken rather than the point being silently dropped.
   */
  values: readonly (number | null)[];
  width?: number;
  height?: number;
  /** Fixed value range; defaults to the data's own min/max. */
  min?: number;
  max?: number;
  /**
   * Whether a rising series is good. `false` inverts the trend colour, which
   * matters for latency and cost — without it a latency regression paints green.
   */
  higherIsBetter?: boolean;
  className?: string;
  title?: string;
}

/**
 * Dependency-free sparkline. Colours come from CSS tokens, so it tracks
 * light/dark automatically.
 *
 * Degenerate series are handled explicitly rather than falling out of the maths:
 * a single point renders as a dot (a lone `M x y` path draws nothing at all),
 * and an all-equal series is centred vertically instead of being pinned to the
 * floor — a 100%-passing suite's trend line used to sit on the bottom edge,
 * which reads as 0%.
 */
export function Sparkline({
  values,
  width = 96,
  height = 26,
  min,
  max,
  higherIsBetter = true,
  className,
  title,
}: SparklineProps) {
  const gradId = useId();

  const finite = values.filter((v): v is number => v != null);
  if (finite.length === 0) {
    return (
      <svg
        width={width}
        height={height}
        className={cn("text-muted", className)}
        aria-hidden
      />
    );
  }

  const pad = 2;
  const w = width - pad * 2;
  const h = height - pad * 2;

  const lo = min ?? Math.min(...finite);
  const hiRaw = max ?? Math.max(...finite);
  // An all-equal series has no range to normalise against; centre it.
  const flat = hiRaw === lo;
  const span = flat ? 1 : hiRaw - lo;

  // x from the index, so a gap keeps its slot. A single point sits centred
  // rather than glued to the left edge.
  const stepX = values.length > 1 ? w / (values.length - 1) : 0;
  const xAt = (i: number) => (values.length > 1 ? pad + i * stepX : pad + w / 2);
  const yAt = (v: number) => (flat ? pad + h / 2 : pad + h - ((v - lo) / span) * h);

  const firstVal = finite[0]!;
  const lastVal = finite[finite.length - 1]!;
  // With fewer than two points there is no trend to report; saying "up" would be
  // an assertion the data doesn't support.
  const rising = lastVal >= firstVal;
  const stroke =
    finite.length < 2
      ? "var(--color-skip)"
      : rising === higherIsBetter
        ? "var(--color-pass)"
        : "var(--color-fail)";

  // One subpath per unbroken run of values.
  const segments: string[] = [];
  let current: string[] = [];
  values.forEach((v, i) => {
    if (v == null) {
      if (current.length > 0) segments.push(current.join(" "));
      current = [];
      return;
    }
    current.push(
      `${current.length === 0 ? "M" : "L"}${xAt(i).toFixed(1)} ${yAt(v).toFixed(1)}`,
    );
  });
  if (current.length > 0) segments.push(current.join(" "));
  const line = segments.join(" ");

  // Explicit accumulator type: inference widens it to `number | null` from the
  // element type otherwise.
  const lastIdx = values.reduce<number>(
    (acc, v, i) => (v != null ? i : acc),
    -1,
  );
  const firstIdx = values.findIndex((v) => v != null);
  const lastPoint = [xAt(lastIdx), yAt(lastVal)] as const;

  // The area fill only reads correctly under a continuous line.
  const hasGap = finite.length !== values.length;
  const area =
    hasGap || finite.length < 2
      ? null
      : `${line} L${xAt(lastIdx).toFixed(1)} ${height - pad} L${xAt(firstIdx).toFixed(1)} ${height - pad} Z`;

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
      {area ? (
        <>
          <defs>
            <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={stroke} stopOpacity="0.22" />
              <stop offset="100%" stopColor={stroke} stopOpacity="0" />
            </linearGradient>
          </defs>
          <path d={area} fill={`url(#${gradId})`} stroke="none" />
        </>
      ) : null}
      {finite.length > 1 ? (
        <path
          d={line}
          fill="none"
          stroke={stroke}
          strokeWidth={1.5}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      ) : null}
      <circle cx={lastPoint[0]} cy={lastPoint[1]} r={2} fill={stroke} />
    </svg>
  );
}
