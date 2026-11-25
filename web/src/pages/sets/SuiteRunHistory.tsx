import { useId, useState } from "react";
import { Link } from "react-router";
import type { RunListItem } from "@/api";
import { isCiRun } from "@/components/RunOriginCell";
import { Card } from "@/components/ui/Card";
import { cn } from "@/lib/cn";
import { formatDateAbsolute, formatPercent, passRate, shortRunId } from "@/lib/format";
import { runPath } from "@/lib/routes";

const WINDOW = 16;
const TICKS = [100, 75, 50, 25, 0];
const MARK = "block size-2.5 border-2 border-surface outline outline-current forced-colors:outline-[CanvasText]";

function OriginMark({ ci }: { ci: boolean }) {
  return <span aria-hidden="true" className={cn(MARK, ci ? "rounded-full bg-accent text-accent" : "rotate-45 bg-surface text-muted")} />;
}

export function SuiteRunHistory({ runs }: { runs: RunListItem[] }) {
  const heading = useId();
  const [hovered, setHovered] = useState<string>();
  const [focused, setFocused] = useState<string>();
  // The query is newest first. Keep the newest window even if the user loads
  // more table rows, and assign every run a slot (including unscored runs).
  const points = runs.slice(0, WINDOW).reverse().map((run, index, window) => ({
    run,
    ci: isCiRun(run),
    rate: passRate(run.pass_count, run.fail_count + (run.xpass_count ?? 0), run.error_count),
    x: (index + 0.5) * 100 / window.length,
  }));
  const active = points.find(({ run }) => run.id === (focused ?? hovered));
  const latest = points.filter(({ rate }) => rate !== null).at(-1);
  const readout = active ?? latest;
  const unscored = points.filter(({ rate }) => rate === null).length;

  let connect = false;
  let ciPath = "";
  for (const { ci, rate, x } of points) {
    if (!ci) continue;
    if (rate === null) {
      connect = false;
      continue;
    }
    ciPath += `${connect ? "L" : "M"} ${x} ${(1 - rate) * 100} `;
    connect = true;
  }

  return (
    <section aria-labelledby={heading}>
      <Card className="space-y-4 bg-surface">
        <header className="flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
          <div>
            <h3 id={heading} className="text-base font-semibold tracking-tight">Pass-rate history</h3>
            <p className="mt-0.5 text-xs text-muted">
              {points.length} recent {points.length === 1 ? "run" : "runs"} in order, not to time scale. Uses the cached-run filter above.
            </p>
          </div>
          {latest ? (
            <ul aria-label="Run origins" className="flex flex-wrap gap-x-4 gap-y-1 pt-1 text-xs text-muted">
              <li className="flex items-center gap-2"><OriginMark ci /> CI</li>
              <li className="flex items-center gap-2"><OriginMark ci={false} /> Local</li>
            </ul>
          ) : null}
        </header>

        {latest ? (
          <>
            <div tabIndex={points.length > 10 ? 0 : undefined} role="group" aria-label="Pass rate by run, oldest to newest" className="scroll-hint overflow-x-auto rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              <div className="grid grid-cols-[1.75rem_minmax(0,1fr)] items-start gap-x-1 px-0.5 pt-3" style={{ minWidth: points.length * 24 + 36 }}>
                <div aria-hidden="true" className="relative h-32 text-right text-[11px] tabular-nums text-muted">
                  {TICKS.map((tick) => <span key={tick} className="absolute right-0 -translate-y-1/2" style={{ top: `${100 - tick}%` }}>{tick}%</span>)}
                </div>
                <div className="relative h-32">
                  {TICKS.map((tick) => <div key={tick} aria-hidden="true" className="absolute inset-x-0 border-t border-border" style={{ top: `${100 - tick}%` }} />)}
                  <svg className="pointer-events-none absolute inset-0 size-full overflow-visible text-accent forced-colors:text-[CanvasText]" viewBox="0 0 100 100" preserveAspectRatio="none" aria-hidden="true">
                    <path d={ciPath} stroke="currentColor" strokeOpacity={0.65} strokeWidth="2" fill="none" vectorEffect="non-scaling-stroke" />
                  </svg>
                  <div className="absolute inset-0 grid" style={{ gridTemplateColumns: `repeat(${points.length}, minmax(0, 1fr))` }} onMouseLeave={() => setHovered(undefined)}>
                    {points.map(({ run, rate, ci }) => rate === null ? <div key={run.id} onMouseEnter={() => setHovered(undefined)} /> : (
                      <Link
                        key={run.id}
                        to={runPath(run.id)}
                        aria-label={`Open run ${run.id}: ${formatPercent(rate)}, ${ci ? "CI" : "local"}, ${formatDateAbsolute(run.created_at)}`}
                        className="relative outline-none"
                        onMouseEnter={() => setHovered(run.id)}
                        onFocus={() => setFocused(run.id)}
                        onBlur={() => setFocused(undefined)}
                      >
                        {active?.run.id === run.id && <span aria-hidden="true" className="pointer-events-none absolute inset-y-0 left-1/2 border-l border-border-strong" />}
                        <span aria-hidden="true" className={cn("pointer-events-none absolute left-1/2 -translate-1/2 rounded-sm", active?.run.id === run.id && "ring-2 ring-ring ring-offset-2 ring-offset-surface")} style={{ top: `${(1 - rate) * 100}%` }}>
                          <OriginMark ci={ci} />
                        </span>
                      </Link>
                    ))}
                  </div>
                </div>
                <div className="col-start-2 mt-3 flex justify-between gap-2 text-[11px] text-muted" aria-hidden="true">
                  <span>Oldest shown</span><span>Newest shown</span>
                </div>
              </div>
            </div>
            <div role="status" className="flex min-h-8 flex-wrap items-center gap-x-3 gap-y-1 border-t border-border pt-3 text-xs text-muted">
              {readout ? (
                <>
                  <strong className="tabular-nums text-fg">{formatPercent(readout.rate)}</strong>
                  <span>{readout.ci ? "CI" : "Local"}</span>
                  <time dateTime={readout.run.created_at}>{formatDateAbsolute(readout.run.created_at)}</time>
                  <span className="break-all font-mono">{shortRunId(readout.run.id)}</span>
                </>
              ) : null}
            </div>
          </>
        ) : <p className="text-sm text-muted">No scored cases in this window.</p>}
        {latest && unscored > 0 && <p className="text-xs text-muted">{unscored} {unscored === 1 ? "run has" : "runs have"} no scored cases. See the table below.</p>}
      </Card>
    </section>
  );
}
