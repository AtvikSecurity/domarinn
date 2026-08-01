import { useState } from "react";
import {
  useSetAccess,
  useSetGrantDeleteMutation,
  useSetGrantMutation,
  useSetRestrictionMutation,
  useUsers,
} from "@/api/queries";
import type { GrantLevel } from "@/api";
import { ApiError } from "@/api/client";
import { useAuthView } from "@/auth/AuthProvider";
import { Button } from "@/components/ui/Button";
import { Chip } from "@/components/ui/Chip";
import { Modal, ModalActions } from "@/components/ui/Modal";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { ErrorState } from "@/components/States";
import { formatDate, isoFromEpoch } from "@/lib/format";

/** Every level a grant can hold, least privileged first (`GrantLevel`). */
const LEVELS: GrantLevel[] = ["view", "upload", "manage"];

const LEVEL_HINT: Record<GrantLevel, string> = {
  view: "Read this set's runs.",
  upload: "Read, and upload runs into it.",
  manage: "Read, upload, and administer this list.",
};

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    if (err.status === 403) {
      return "Your credential may read this list but not change it.";
    }
    if (err.status === 404) {
      // The set gate refuses with 404 rather than 403 — the list names who can
      // reach a restricted set, so both answers have to look the same.
      return "You no longer manage this set, or that account no longer exists.";
    }
    if (err.status === 401) {
      return "Your session has expired. Sign in again to make changes.";
    }
  }
  return fallback;
}

/**
 * Who may reach one set, and whether it is restricted at all.
 *
 * The two `restricted` answers are both here, and they are not interchangeable.
 * `coveringRestricted` (from the browse payload, passed in) is whether ANY
 * restriction reaches this set, so it is what the status chip and the sentence
 * under it must say — a suite inside a locked project is hidden, and a panel
 * reading its own exact-scope flag would tell its reader "anyone who can read
 * this server can see this set's runs", which is false. `access.data.restricted`
 * is the EXACT-scope row this panel's toggle owns, so it drives only the
 * toggle's verb and the confirm copy: what pressing the button would write.
 *
 * The affordances are gated on TWO independent things, because the server is:
 * holding `manage` is what lets you read this list (read scope), and write scope
 * is what lets you change it. A viewer-role account with a manage grant can open
 * this panel and would be 403'd by every mutation in it, so it renders read-only
 * rather than offering controls that cannot work.
 */
export function AccessPanel({
  project,
  suite,
  coveringRestricted,
  open,
  onClose,
}: {
  project: string;
  /** `null` for the project-wide list. */
  suite: string | null;
  /**
   * Whether any restriction reaches this set — the COVERING answer, which only
   * the browse payload knows. Both call sites already hold it.
   */
  coveringRestricted: boolean;
  open: boolean;
  onClose: () => void;
}) {
  const view = useAuthView();
  const access = useSetAccess(project, suite, { enabled: open });
  const grantMutation = useSetGrantMutation(project, suite);
  const removeMutation = useSetGrantDeleteMutation(project, suite);
  const restrictionMutation = useSetRestrictionMutation(project, suite);
  // Admin-only endpoint; asking for it as a manager would 403 on every open.
  const users = useUsers(open && view.canAdmin);

  const [banner, setBanner] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [newUserId, setNewUserId] = useState("");
  const [newLevel, setNewLevel] = useState<GrantLevel>("view");

  const label = suite === null ? project : `${project} / ${suite}`;
  // What this set's OWN row says — the only thing the toggle can write.
  const restrictedHere = access.data?.restricted ?? false;
  // Hidden, but by somebody else's row: a suite under a locked project.
  const inherited = coveringRestricted && !restrictedHere;
  const scope = suite === null ? "project" : "suite";
  const grants = access.data?.grants ?? [];
  const granted = new Set(grants.map((g) => g.user_id));
  const addable = (users.data ?? []).filter((u) => !granted.has(u.id));

  function close() {
    setBanner(null);
    setConfirming(false);
    setNewUserId("");
    setNewLevel("view");
    onClose();
  }

  async function changeLevel(userId: string, level: GrantLevel) {
    setBanner(null);
    try {
      await grantMutation.mutateAsync({ userId, level });
    } catch (err) {
      setBanner(errorMessage(err, "Could not change that level."));
    }
  }

  async function remove(userId: string) {
    setBanner(null);
    try {
      await removeMutation.mutateAsync(userId);
    } catch (err) {
      setBanner(errorMessage(err, "Could not remove that grant."));
    }
  }

  async function addGrant() {
    if (!newUserId) return;
    setBanner(null);
    try {
      await grantMutation.mutateAsync({ userId: newUserId, level: newLevel });
      setNewUserId("");
      setNewLevel("view");
    } catch (err) {
      setBanner(errorMessage(err, "Could not add that person."));
    }
  }

  async function toggleRestriction() {
    setBanner(null);
    try {
      await restrictionMutation.mutateAsync(!restrictedHere);
      setConfirming(false);
    } catch (err) {
      setConfirming(false);
      setBanner(errorMessage(err, "Could not change the restriction."));
    }
  }

  return (
    <Modal
      open={open}
      onOpenChange={(next) => !next && close()}
      title={`Access · ${label}`}
      description={
        suite === null
          ? "Grants on a project cover every suite in it."
          : "Grants on a suite cover only that suite."
      }
      size="lg"
      footer={
        confirming ? (
          <ModalActions
            onCancel={() => setConfirming(false)}
            confirmLabel={
              restrictedHere ? `Unlock ${scope}` : `Restrict ${scope}`
            }
            confirmVariant="danger"
            pending={restrictionMutation.isPending}
            onConfirm={() => void toggleRestriction()}
          />
        ) : undefined
      }
    >
      <div className="space-y-4">
        {banner ? (
          <div
            role="alert"
            className="rounded-md border border-fail/30 bg-fail/5 px-3 py-2 text-sm text-fail"
          >
            {banner}
          </div>
        ) : null}

        {access.isPending ? (
          <CenteredSpinner label="Loading access…" />
        ) : access.isError ? (
          <ErrorState error={access.error} onRetry={() => access.refetch()} />
        ) : (
          <>
            <section className="rounded-lg border border-border bg-bg/40 px-3 py-2.5">
              <div className="flex flex-wrap items-center gap-2">
                {/* "Visibility" rather than repeating the chip's word in
                    bold beside it — the chip IS the status. */}
                <span className="text-sm font-medium">Visibility</span>
                {/* The COVERING answer: whether this set is hidden at all. Its
                    own row is a different question, answered one line down. */}
                <Chip tone={coveringRestricted ? "amber" : "neutral"} size="xs">
                  {coveringRestricted ? "restricted" : "open"}
                </Chip>
                {view.canAdmin && !confirming ? (
                  <Button
                    className="ml-auto"
                    variant="secondary"
                    size="sm"
                    onClick={() => setConfirming(true)}
                  >
                    {restrictedHere ? `Unlock ${scope}` : `Restrict ${scope}`}
                  </Button>
                ) : null}
              </div>
              <p className="mt-1 text-xs text-muted">
                {coveringRestricted
                  ? "Only the people below can see this set's runs."
                  : "Anyone who can read this server can see this set's runs."}
              </p>
              {inherited ? (
                <p className="mt-1 text-xs text-muted">
                  The restriction is inherited from {project}; this suite has
                  none of its own, and grants on the project count too.
                </p>
              ) : null}
              {confirming ? (
                <p className="mt-2 rounded-md border border-amber/30 bg-amber/5 px-2.5 py-1.5 text-xs text-amber">
                  {/* Exact scope: this describes the row the button writes,
                      not what covers the set afterwards. */}
                  {restrictedHere
                    ? `Unlocking removes this ${scope}'s own restriction. The grants below are kept.`
                    : inherited
                      ? `${suite} is already hidden by ${project}'s restriction. Adding its own keeps it hidden even if ${project} is unlocked.`
                      : `Restricting hides this ${scope} from everyone except the people below — including anyone whose page is open right now.`}
                </p>
              ) : null}
            </section>

            {!view.canWrite ? (
              <p className="text-xs text-muted">
                You hold this set, but you are signed in with a read-only
                credential, so this list is view-only.
              </p>
            ) : null}

            <section>
              <h3 className="text-sm font-semibold">People</h3>
              {grants.length === 0 ? (
                <p className="mt-2 text-sm text-muted">
                  Nobody has been granted this set yet.
                  {coveringRestricted && !inherited
                    ? " While it is restricted, only admins can reach it."
                    : null}
                </p>
              ) : (
                <div className="relative mt-2 overflow-x-auto">
                  {/* `relative` above: contains the trailing Actions column's
                      `sr-only` header, which is `position: absolute` and
                      escapes a scroller that is not a containing block. See
                      SetsPage for the full account. */}
                  <table className="w-full min-w-[420px] text-sm">
                    <thead>
                      <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                        <th className="py-2 pr-3 font-medium">Person</th>
                        <th className="px-3 py-2 font-medium">Level</th>
                        <th className="py-2 pl-3 text-right font-medium">
                          <span className="sr-only">Actions</span>
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {grants.map((g) => (
                        <tr
                          key={g.user_id}
                          data-testid={`grant-row-${g.username}`}
                          className="border-b border-border/60 last:border-0"
                        >
                          <td className="py-2 pr-3">
                            <div className="font-medium">{g.username}</div>
                            <div className="text-[11px] text-muted">
                              added {formatDate(isoFromEpoch(g.created_at))}
                              {g.created_by ? ` by ${g.created_by}` : null}
                            </div>
                          </td>
                          <td className="px-3 py-2">
                            {view.canWrite ? (
                              <select
                                aria-label={`Level for ${g.username}`}
                                className="h-8 rounded-md border border-border bg-bg px-2 text-sm outline-none focus:ring-2 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
                                value={g.level}
                                disabled={grantMutation.isPending}
                                onChange={(e) =>
                                  void changeLevel(
                                    g.user_id,
                                    e.target.value as GrantLevel,
                                  )
                                }
                              >
                                {LEVELS.map((l) => (
                                  <option key={l} value={l} title={LEVEL_HINT[l]}>
                                    {l}
                                  </option>
                                ))}
                              </select>
                            ) : (
                              <Chip size="xs" title={LEVEL_HINT[g.level]}>
                                {g.level}
                              </Chip>
                            )}
                          </td>
                          <td className="py-2 pl-3 text-right">
                            {view.canWrite ? (
                              <Button
                                variant="ghost"
                                size="sm"
                                className="text-fail hover:bg-fail/10 hover:text-fail"
                                // Bare verb in the row, the person's name only
                                // in the label — the app's convention (see the
                                // admin page's per-row actions).
                                aria-label={`Remove ${g.username}`}
                                disabled={removeMutation.isPending}
                                onClick={() => void remove(g.user_id)}
                              >
                                Remove
                              </Button>
                            ) : null}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </section>

            {view.canAdmin && view.canWrite ? (
              <section className="flex flex-wrap items-end gap-2 border-t border-border pt-4">
                <label className="flex flex-col gap-1">
                  <span className="text-xs font-medium text-fg">Add person</span>
                  <select
                    aria-label="Add person"
                    className="h-8 min-w-[10rem] rounded-md border border-border bg-bg px-2 text-sm outline-none focus:ring-2 focus:ring-ring"
                    value={newUserId}
                    onChange={(e) => setNewUserId(e.target.value)}
                  >
                    <option value="">Choose an account…</option>
                    {addable.map((u) => (
                      <option key={u.id} value={u.id}>
                        {u.username}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="flex flex-col gap-1">
                  <span className="text-xs font-medium text-fg">Level</span>
                  <select
                    aria-label="Level for the new grant"
                    className="h-8 rounded-md border border-border bg-bg px-2 text-sm outline-none focus:ring-2 focus:ring-ring"
                    value={newLevel}
                    onChange={(e) => setNewLevel(e.target.value as GrantLevel)}
                  >
                    {LEVELS.map((l) => (
                      <option key={l} value={l} title={LEVEL_HINT[l]}>
                        {l}
                      </option>
                    ))}
                  </select>
                </label>
                <Button
                  variant="primary"
                  size="sm"
                  disabled={!newUserId || grantMutation.isPending}
                  onClick={() => void addGrant()}
                >
                  Add
                </Button>
              </section>
            ) : view.canWrite ? (
              <p className="border-t border-border pt-4 text-xs text-muted">
                Ask an admin to add new people; you can adjust or remove
                existing grants.
              </p>
            ) : null}
          </>
        )}
      </div>
    </Modal>
  );
}
