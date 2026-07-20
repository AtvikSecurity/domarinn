import { useCaseDetail } from "@/api/queries";
import { Spinner } from "@/components/ui/Spinner";
import { DiffView } from "./DiffView";

function outputString(v: string | Record<string, unknown> | undefined): string {
  if (v === undefined) return "";
  return typeof v === "string" ? v : JSON.stringify(v, null, 2);
}

export function CompareRowExpansion({
  baseRunId,
  headRunId,
  caseKey,
}: {
  baseRunId: string;
  headRunId: string;
  caseKey: string;
}) {
  const base = useCaseDetail(baseRunId, caseKey);
  const head = useCaseDetail(headRunId, caseKey);

  const loading = base.isPending || head.isPending;

  return (
    <div className="border-t border-border bg-bg/40 px-4 py-3">
      {loading ? (
        <div className="flex items-center gap-2 p-3 text-xs text-muted">
          <Spinner /> Loading outputs…
        </div>
      ) : (
        <DiffView
          base={outputString(base.data?.output)}
          head={outputString(head.data?.output)}
        />
      )}
    </div>
  );
}
