import { useMemo, useState } from "react";
import {
  useCreateUser,
  useDeleteUser,
  useUpdateUser,
  useUsers,
} from "@/api/queries";
import type { Role, UserView } from "@/api";
import { ApiError } from "@/api/client";
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

function errorMessage(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    if (err.status === 409) {
      const body = err.body as { error?: string } | undefined;
      if (body?.error === "last_admin") {
        return "Cannot remove the last active admin.";
      }
      if (body?.error === "username_taken") {
        return "That username is already taken.";
      }
      return "Conflict — the change was rejected by the server.";
    }
    if (err.status === 403) return "You are not allowed to perform this action.";
  }
  return fallback;
}

export function AdminPage() {
  const view = useAuthView();
  const usersQuery = useUsers();
  const updateUser = useUpdateUser();
  const deleteUser = useDeleteUser();

  const [banner, setBanner] = useState<string | null>(null);
  const [resetting, setResetting] = useState<UserView | null>(null);
  const [deleting, setDeleting] = useState<UserView | null>(null);
  const colPrefs = useColumnPrefs(USERS_TABLE_ID);
  const shownCols = useMemo(
    () => visibleColumns(USER_COLUMNS, colPrefs),
    [colPrefs],
  );
  const shownIds = useMemo(() => new Set(shownCols.map((c) => c.id)), [shownCols]);
  const sort = useSortParam();

  async function changeRole(user: UserView, role: Role) {
    setBanner(null);
    try {
      await updateUser.mutateAsync({ id: user.id, patch: { role } });
    } catch (err) {
      setBanner(errorMessage(err, "Could not change the role."));
    }
  }

  async function toggleDisabled(user: UserView) {
    setBanner(null);
    try {
      await updateUser.mutateAsync({
        id: user.id,
        patch: { disabled: !user.disabled },
      });
    } catch (err) {
      setBanner(errorMessage(err, "Could not update the account."));
    }
  }

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

      <CreateUserForm onError={(m) => setBanner(m)} />

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
        onError={(m) => setBanner(m)}
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
              setBanner(null);
              try {
                await deleteUser.mutateAsync(deleting.id);
                setDeleting(null);
              } catch (err) {
                setDeleting(null);
                setBanner(errorMessage(err, "Could not delete the user."));
              }
            }}
          />
        }
      />
    </div>
  );
}

function CreateUserForm({ onError }: { onError: (message: string) => void }) {
  const createUser = useCreateUser();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("member");

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    onError("");
    try {
      await createUser.mutateAsync({
        username: username.trim(),
        password,
        role,
      });
      setUsername("");
      setPassword("");
      setRole("member");
    } catch (err) {
      onError(errorMessage(err, "Could not create the user."));
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

function ResetPasswordModal({
  user,
  onClose,
  onError,
}: {
  user: UserView | null;
  onClose: () => void;
  onError: (message: string) => void;
}) {
  const updateUser = useUpdateUser();
  const [password, setPassword] = useState("");

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!user) return;
    try {
      await updateUser.mutateAsync({ id: user.id, patch: { password } });
      setPassword("");
      onClose();
    } catch (err) {
      onError(errorMessage(err, "Could not reset the password."));
    }
  }

  return (
    <Modal
      open={user !== null}
      onOpenChange={(o) => {
        if (!o) {
          setPassword("");
          onClose();
        }
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
        <ModalActions
          onCancel={() => {
            setPassword("");
            onClose();
          }}
          confirmLabel="Set password"
          confirmType="submit"
          pending={updateUser.isPending}
        />
      </form>
    </Modal>
  );
}
