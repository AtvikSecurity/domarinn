import { useState } from "react";
import { useClearBaseline } from "@/api/queries";
import { apiErrorMessage } from "@/api/errorMessage";
import { Button } from "@/components/ui/Button";
import { Modal, ModalActions } from "@/components/ui/Modal";

/**
 * Unpin a suite's comparison baseline, behind a confirm.
 *
 * Shared by the suite page and the run page, which both show the pin. Callers
 * decide whether to render it at all — see `canPinBaseline`.
 */
export function UnpinBaselineButton({
  project,
  suite,
  pinned,
}: {
  project: string;
  suite: string;
  /** What is pinned, for the confirm text: a run id or `branch <name>`. */
  pinned: string;
}) {
  const clear = useClearBaseline(project, suite);
  const [open, setOpen] = useState(false);

  return (
    <>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => {
          // A failure from an earlier attempt is not this attempt's answer.
          clear.reset();
          setOpen(true);
        }}
        disabled={clear.isPending}
      >
        Unpin baseline
      </Button>
      <Modal
        open={open}
        onOpenChange={setOpen}
        title="Unpin this suite's baseline?"
        description={
          <>
            <span className="font-mono">{pinned}</span> stops being the
            comparison baseline for {project}/{suite}. The suite has no
            baseline until a run or branch is pinned again.
          </>
        }
        footer={
          <ModalActions
            onCancel={() => setOpen(false)}
            onConfirm={() =>
              clear.mutate(undefined, { onSuccess: () => setOpen(false) })
            }
            confirmLabel="Unpin baseline"
            confirmVariant="danger"
            pending={clear.isPending}
          />
        }
      >
        {clear.isError ? (
          <p role="alert" className="text-sm text-fail">
            {apiErrorMessage(clear.error, "Could not unpin the baseline. Try again.")}
          </p>
        ) : null}
      </Modal>
    </>
  );
}
