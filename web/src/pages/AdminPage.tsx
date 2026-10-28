import { useMemo, useState } from "react";
import {
  useCreateUser,
  useDeleteUser,
  useUpdateUser,
  useUsers,
} from "@/api/queries";
import type { Role, UserView } from "@/api";
import { apiErrorMessage } from "@/api/errorMessage";
import { useAuthView } from "@/auth/AuthProvider";
import { ALL_ROLES } from "@/lib/authz";
import { formatDate, parseTimestamp } from "@/lib/format";
import { type SortAccessor, sortRows } from "@/lib/sort";
import { useSortParam } from "@/lib/useTableSort";
import { Button } from "@/components/ui/Button";
import { Card } from "@/components/ui/Card";
import { Chip } from "@/components/ui/Chip";
import { CHROME_FRAME } from "@/components/ui/chrome";
import { Modal, ModalActions } from "@/components/ui/Modal";
import { TextField } from "@/components/ui/TextField";
import { CenteredSpinner } from "@/components/ui/Spinner";
import { EmptyState, ErrorState } from "@/components/States";
import { ProviderBadge } from "@/components/ProviderBadge";
import { ColumnGroup } from "@/components/ui/ColumnGroup";
import { ColumnPicker } from "@/components/ui/ColumnPicker";
import { ResizableTh } from "@/components/ui/ResizableTh";
import { type ColumnDef, visibleColumns } from "@/lib/tableColumns";
import { resetColumns, setColumnVisible, useColumnPrefs } from "@/lib/useColumnPrefs";
import { cn } from "@/lib/cn";

const USERS_TABLE_ID = "admin";

/**
 * Three of the five are structural. `status` is what says an account is
 * disabled — the single most consequential fact on the row — and `actions` is
 * the only way to act on it; hiding either turns this from an administration
 * surface into a list.
 */
const USER_COLUMNS: ColumnDef[] = [
  { id: "username", label: "Username", track: "auto", min: 160, alwaysVisible: true },
  { id: "role", label: "Role", track: "140px", min: 120 },
  { id: "status", label: "Status", track: "100px", min: 90, alwaysVisible: true },
  { id: "created", label: "Created", track: "140px", min: 110 },
  {
    id: "actions",
    label: "Actions",
    track: "300px",
    min: 280,
    numeric: true,
    alwaysVisible: true,
  },
];

/** Sortable columns; the actions column is buttons, not data. Role sorts by
 *  privilege rank (`ALL_ROLES` order), not alphabetically. */
const USER_SORT_FIELDS: Record<string, SortAccessor<UserView>> = {
  username: (u) => u.username,
  role: (u) => ALL_ROLES.indexOf(u.role),
  status: (u) => (u.disabled ? 1 : 0),
  created: (u) => parseTimestamp(u.created_at),
};

/**
 * A role or status change held back for confirmation. The server only refuses
 * the change that would leave no enabled admin, so an admin who has a second
 * admin can still disable or demote themselves in one click — and lose this
 * page with it. Disabling anyone signs them out, so that asks too.
 */
type PendingChange =
  | { kind: "role"; user: UserView; role: Role }
  | { kind: "disable"; user: UserView };

/** Lower index in `ALL_ROLES` is less privilege. */
function isDemotion(from: Role, to: Role): boolean {
  return ALL_ROLES.indexOf(to) < ALL_ROLES.indexOf(from);
}

function confirmCopy(change: PendingChange, isSelf: boolean) {
  if (change.kind === "role") {
    return {
      title: "Change your own role",
      description:
        `You are signed in as "${change.user.username}". Changing your role ` +
        `from ${change.user.role} to ${change.role} takes effect immediately, ` +
        "and you lose access to this page.",
      confirmLabel: `Change to ${change.role}`,
    };
  }
  return {
    title: isSelf ? "Disable your own account" : "Disable user",
    description: isSelf
      ? `You are signed in as "${change.user.username}". Disabling this ` +
        "account signs you out, and only another admin can enable it again."
      : `Disable "${change.user.username}"? They are signed out and cannot ` +
        "sign in until an admin enables the account again.",
    confirmLabel: isSelf ? "Disable my account" : "Disable user",
  };
}

export function AdminPage() {
  const view = useAuthView();
  const usersQuery = useUsers();
  const updateUser = useUpdateUser();
  const deleteUser = useDeleteUser();

  const [banner, setBanner] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [resetting, setResetting] = useState<UserView | null>(null);
  const [deleting, setDeleting] = useState<UserView | null>(null);
  const [confirming, setConfirming] = useState<PendingChange | null>(null);
  const selfId = view.user?.id ?? null;
  const colPrefs = useColumnPrefs(USERS_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(USER_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownIds = useMemo(() => new Set(shownCols.map((c) => c.id)), [shownCols]);
  const sort = useSortParam();

  function clearMessages() {
    setBanner(null);
    setNotice(null);
  }

  async function applyRole(user: UserView, role: Role) {
    clearMessages();
    try {
      await updateUser.mutateAsync({ id: user.id, patch: { role } });
    } catch (err) {
      setBanner(apiErrorMessage(err, "Could not change the role."));
    }
  }

  async function applyDisabled(user: UserView, disabled: boolean) {
    clearMessages();
    try {
      await updateUser.mutateAsync({ id: user.id, patch: { disabled } });
    } catch (err) {
      setBanner(apiErrorMessage(err, "Could not update the account."));
    }
  }

  function changeRole(user: UserView, role: Role) {
    if (user.id === selfId && isDemotion(user.role, role)) {
      setConfirming({ kind: "role", user, role });
      return;
    }
    void applyRole(user, role);
  }

  function toggleDisabled(user: UserView) {
    if (!user.disabled) {
      setConfirming({ kind: "disable", user });
      return;
    }
    void applyDisabled(user, false);
  }

  async function confirmChange() {
    if (!confirming) return;
    const change = confirming;
    if (change.kind === "role") await applyRole(change.user, change.role);
    else await applyDisabled(change.user, true);
    setConfirming(null);
  }

  const confirmText = confirming
    ? confirmCopy(confirming, confirming.user.id === selfId)
    : null;

  return (
    <div className="max-w-4xl space-y-5">
      <div>
        <h1 className="text-lg font-semibold tracking-tight">Users</h1>
        <p className="text-sm text-muted">
          Manage accounts, roles, and access. Signed in as{" "}
          <span className="text-fg">{view.user?.username ?? "admin"}</span>.
        </p>
      </div>

      {banner ? (
        <div
          role="alert"
          className="rounded-md border border-fail/30 bg-fail/5 px-3 py-2 text-sm text-fail"
        >
          {banner}
        </div>
      ) : null}

      {notice ? (
        <div
          role="status"
          className="rounded-md border border-pass/30 bg-pass/5 px-3 py-2 text-sm text-pass"
        >
          {notice}
        </div>
      ) : null}

      <CreateUserForm
        onStart={clearMessages}
        onError={(m) => setBanner(m)}
        onCreated={(name) => setNotice(`Created user "${name}".`)}
      />

      <section className={cn(CHROME_FRAME, "overflow-hidden")}>
        <header className="border-b border-border px-4 py-2.5 text-sm font-semibold">
          Accounts
        </header>
        {usersQuery.isPending ? (
          <CenteredSpinner label="Loading users…" />
        ) : usersQuery.isError ? (
          <div className="p-4">
            <ErrorState
              error={usersQuery.error}
              onRetry={() => usersQuery.refetch()}
            />
          </div>
        ) : usersQuery.data.length === 0 ? (
          <div className="p-4">
            <EmptyState title="No users" />
          </div>
        ) : (
          <>
          <div className="flex justify-end px-4 pt-3">
            <ColumnPicker
              columns={USER_COLUMNS}
              prefs={colPrefs}
              onChange={(id, visible) =>
                setColumnVisible(USERS_TABLE_ID, id, visible)
              }
              onReset={() => resetColumns(USERS_TABLE_ID)}
            />
          </div>
          <div className="overflow-x-auto scroll-hint [--scroll-hint-bg:var(--bg)]">
            {/* 840px is the declared tracks' sum, and has to stay under the
                page's own `max-w-4xl` (896px) — a floor above it would clip
                the row actions against the card rather than scroll to them. */}
            <table className="w-full min-w-[840px] table-fixed text-sm">
              <ColumnGroup columns={USER_COLUMNS} prefs={colPrefs} />
              <thead>
                <tr className="border-b border-border text-left text-xs uppercase tracking-wide text-muted">
                  {shownCols.map((c, i, arr) => (
                    <ResizableTh
                      isLast={i === arr.length - 1}
                      key={c.id}
                      def={c}
                      tableId={USERS_TABLE_ID}
                      prefs={colPrefs}
                      sort={
                        USER_SORT_FIELDS[c.id]
                          ? {
                              active: sort.sortFor(c.id),
                              onToggle: () => sort.toggle(c.id),
                            }
                          : undefined
                      }
                      className={cn(
                        "py-2 font-medium",
                        c.id === "username" ? "px-4" : "px-3",
                        c.numeric && "text-right",
                      )}
                    >
                      {c.label}
                    </ResizableTh>
                  ))}
                </tr>
              </thead>
              <tbody>
                {sortRows(usersQuery.data, sort.sorting, USER_SORT_FIELDS).map((u) => (
                  <tr
                    key={u.id}
                    data-testid={`user-row-${u.username}`}
                    className="border-b border-border/60 last:border-0"
                  >
                    {shownIds.has("username") && (
                    <td className="px-4 py-2 font-medium">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span>{u.username}</span>
                        {u.identities.map((identity) => (
                          <ProviderBadge
                            key={`${identity.provider}:${identity.subject}`}
                            identity={identity}
                          />
                        ))}
                      </div>
                    </td>
                    )}
                    {shownIds.has("role") && (
                    <td className="px-3 py-2">
                      <select
                        aria-label={`Role for ${u.username}`}
                        className="h-8 rounded-md border border-border bg-bg px-2 text-sm outline-none focus:ring-2 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-60"
                        value={u.role}
                        disabled={
                          updateUser.isPending || u.role_managed_by !== null
                        }
                        title={
                          u.role_managed_by !== null
                            ? "Role is managed by the identity provider and re-synced on each SSO login."
                            : undefined
                        }
                        onChange={(e) =>
                          changeRole(u, e.target.value as Role)
                        }
                      >
                        {ALL_ROLES.map((r) => (
                          <option key={r} value={r}>
                            {r}
                          </option>
                        ))}
                      </select>
                    </td>
                    )}
                    {shownIds.has("status") && (
                    <td className="px-3 py-2">
                      <Chip tone={u.disabled ? "neutral" : "pass"}>
                        {u.disabled ? "disabled" : "active"}
                      </Chip>
                    </td>
                    )}
                    {shownIds.has("created") && (
                    <td className="truncate px-3 py-2 text-muted">
                      {formatDate(u.created_at)}
                    </td>
                    )}
                    {shownIds.has("actions") && (
                    <td className="px-3 py-2">
                      <div className="flex items-center justify-end gap-2">
                        <Button
                          variant="secondary"
                          size="sm"
                          disabled={updateUser.isPending}
                          onClick={() => toggleDisabled(u)}
                        >
                          {u.disabled ? "Enable" : "Disable"}
                        </Button>
                        <Button
                          variant="secondary"
                          size="sm"
                          disabled={!u.has_password}
                          title={
                            u.has_password
                              ? undefined
                              : "SSO-only account — no local password to reset."
                          }
                          onClick={() => setResetting(u)}
                        >
                          Reset password
                        </Button>
                        <Button
                          variant="danger"
                          size="sm"
                          onClick={() => setDeleting(u)}
                        >
                          Delete
                        </Button>
                      </div>
                    </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          </>
        )}
      </section>

      <ResetPasswordModal
        user={resetting}
        onClose={() => setResetting(null)}
        onReset={(u) => {
          clearMessages();
          setNotice(`Password reset for "${u.username}".`);
        }}
      />

      <Modal
        open={confirming !== null}
        onOpenChange={(o) => !o && setConfirming(null)}
        title={confirmText?.title ?? "Confirm"}
        description={confirmText?.description}
        footer={
          <ModalActions
            onCancel={() => setConfirming(null)}
            confirmLabel={confirmText?.confirmLabel ?? "Confirm"}
            confirmVariant="danger"
            pending={updateUser.isPending}
            onConfirm={confirmChange}
          />
        }
      />

      <Modal
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title="Delete user"
        description={
          deleting
            ? `Permanently delete "${deleting.username}"? This cannot be undone.`
            : undefined
        }
        footer={
          <ModalActions
            onCancel={() => setDeleting(null)}
            confirmLabel="Delete user"
            confirmVariant="danger"
            pending={deleteUser.isPending}
            onConfirm={async () => {
              if (!deleting) return;
              clearMessages();
              try {
                await deleteUser.mutateAsync(deleting.id);
                setDeleting(null);
              } catch (err) {
                setDeleting(null);
                setBanner(apiErrorMessage(err, "Could not delete the user."));
              }
            }}
          />
        }
      />
    </div>
  );
}

function CreateUserForm({
  onStart,
  onError,
  onCreated,
}: {
  onStart: () => void;
  onError: (message: string) => void;
  onCreated: (username: string) => void;
}) {
  const createUser = useCreateUser();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("member");

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    onStart();
    try {
      const created = await createUser.mutateAsync({
        username: username.trim(),
        password,
        role,
      });
      setUsername("");
      setPassword("");
      setRole("member");
      onCreated(created.username);
    } catch (err) {
      onError(apiErrorMessage(err, "Could not create the user."));
    }
  }

  return (
    <Card as="section">
      <h2 className="text-[10px] font-medium uppercase tracking-wide text-muted">
        Create user
      </h2>
      <form onSubmit={onSubmit} className="mt-3 flex flex-wrap items-end gap-3">
        <div className="min-w-[10rem] flex-1">
          <TextField
            label="Username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="off"
            required
          />
        </div>
        <div className="min-w-[10rem] flex-1">
          <TextField
            label="Password"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="new-password"
            required
          />
        </div>
        <label className="flex flex-col gap-1">
          <span className="text-sm font-medium text-fg">Role</span>
          <select
            aria-label="New user role"
            className="h-9 rounded-md border border-border bg-bg px-2 text-sm outline-none focus:ring-2 focus:ring-ring"
            value={role}
            onChange={(e) => setRole(e.target.value as Role)}
          >
            {ALL_ROLES.map((r) => (
              <option key={r} value={r}>
                {r}
              </option>
            ))}
          </select>
        </label>
        <Button
          type="submit"
          variant="primary"
          disabled={createUser.isPending || !username.trim() || !password}
        >
          {createUser.isPending ? "Creating…" : "Create user"}
        </Button>
      </form>
    </Card>
  );
}

/**
 * The refusal renders inside the dialog: it is still open over the page when
 * the request fails, so the page banner would sit behind the overlay where
 * nobody can read it.
 */
function ResetPasswordModal({
  user,
  onClose,
  onReset,
}: {
  user: UserView | null;
  onClose: () => void;
  onReset: (user: UserView) => void;
}) {
  const updateUser = useUpdateUser();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);

  function close() {
    setPassword("");
    setError(null);
    onClose();
  }

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!user) return;
    setError(null);
    try {
      await updateUser.mutateAsync({ id: user.id, patch: { password } });
      close();
      onReset(user);
    } catch (err) {
      setError(apiErrorMessage(err, "Could not reset the password."));
    }
  }

  return (
    <Modal
      open={user !== null}
      onOpenChange={(o) => {
        if (!o) close();
      }}
      title="Reset password"
      description={user ? `Set a new password for "${user.username}".` : undefined}
    >
      <form onSubmit={onSubmit} className="space-y-4">
        <TextField
          label="New password"
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoComplete="new-password"
          autoFocus
          required
        />
        {error ? (
          <p role="alert" className="text-sm text-fail">
            {error}
          </p>
        ) : null}
        <ModalActions
          onCancel={close}
          confirmLabel="Set password"
          confirmType="submit"
          pending={updateUser.isPending}
        />
      </form>
    </Modal>
  );
}
