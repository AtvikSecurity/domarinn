//! Local accounts, sessions, and API keys.
//!
//! These tables live in the runs database (see [`super::schema`]) so they share
//! its writer-mutex + reader-pool. Passwords are stored as argon2 PHC strings;
//! session tokens and API keys are stored only as sha256 hashes (the hashing
//! itself happens in [`crate::auth`] before values reach this layer).
//!
//! Every method runs through the same [`super::Db`] `read`/`write` helpers as
//! the rest of storage, so all SQL executes inside `spawn_blocking`.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::{now_ms, Storage};
use crate::auth::Scope;
use crate::domain::{ApiKeyId, Role, UserId};

// ---------------------------------------------------------------------------
// Row / view types
// ---------------------------------------------------------------------------

/// A full user record, including the argon2 password hash. Kept crate-internal;
/// route handlers project it to a hash-free JSON view before responding.
#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: UserId,
    pub username: String,
    pub password_hash: String,
    pub role: Role,
    pub disabled: bool,
    pub created_at: i64,
    /// Only populated for SSO-provisioned users; local accounts have no email.
    pub email: Option<String>,
}

/// The user behind a valid session (no secrets).
#[derive(Debug, Clone)]
pub struct SessionUser {
    pub user_id: UserId,
    pub username: String,
    pub role: Role,
}

/// The resolved principal behind a valid API key.
#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    pub id: ApiKeyId,
    pub user_id: UserId,
    pub username: String,
    pub role: Role,
    pub scope: Scope,
}

/// API-key metadata for listing (never includes the secret or its hash).
#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub id: ApiKeyId,
    pub user_id: UserId,
    pub name: Option<String>,
    pub prefix: String,
    pub scope: Scope,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
}

/// Outcome of [`Storage::delete_user`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteUserOutcome {
    Deleted,
    NotFound,
    /// The target is the only *enabled* admin; deletion is refused.
    LastAdmin,
}

/// Outcome of [`Storage::update_role_and_disabled`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateUserOutcome {
    Updated,
    NotFound,
    /// The change would leave the instance with no enabled admin; refused.
    LastAdmin,
}

/// Mint a fresh id (a ULID) for the id newtypes used in this module and in
/// [`super::sso`].
pub(super) fn new_id<T: From<String>>() -> T {
    T::from(ulid::Ulid::generate().to_string())
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

impl Storage {
    /// Create a user, returning the stored row. Returns `Ok(None)` if the
    /// username is already taken.
    pub async fn create_user(
        &self,
        username: String,
        password_hash: String,
        role: Role,
    ) -> anyhow::Result<Option<UserRow>> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let taken = tx
                    .query_row(
                        "SELECT 1 FROM users WHERE username = ?1",
                        params![username],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if taken {
                    return Ok(None);
                }
                let id: UserId = new_id();
                let created_at = now_ms();
                tx.execute(
                    "INSERT INTO users (id, username, password_hash, role, disabled, created_at)
                     VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                    params![id, username, password_hash, role, created_at],
                )?;
                tx.commit()?;
                Ok(Some(UserRow {
                    id,
                    username,
                    password_hash,
                    role,
                    disabled: false,
                    created_at,
                    email: None,
                }))
            })
            .await
    }

    pub async fn get_user_by_username(&self, username: String) -> anyhow::Result<Option<UserRow>> {
        self.runs
            .read(move |conn| {
                get_user(conn, "SELECT id, username, password_hash, role, disabled, created_at, email FROM users WHERE username = ?1", &username)
            })
            .await
    }

    pub async fn get_user_by_id(&self, id: UserId) -> anyhow::Result<Option<UserRow>> {
        self.runs
            .read(move |conn| {
                get_user(conn, "SELECT id, username, password_hash, role, disabled, created_at, email FROM users WHERE id = ?1", &id)
            })
            .await
    }

    pub async fn list_users(&self) -> anyhow::Result<Vec<UserRow>> {
        self.runs
            .read(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, username, password_hash, role, disabled, created_at, email
                     FROM users ORDER BY created_at ASC",
                )?;
                let rows = stmt.query_map([], row_to_user)?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
    }

    pub async fn count_users(&self) -> anyhow::Result<i64> {
        self.runs
            .read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?))
            .await
    }

    pub async fn set_user_role(&self, id: UserId, role: Role) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "UPDATE users SET role = ?2 WHERE id = ?1",
                    params![id, role],
                )?;
                Ok(n > 0)
            })
            .await
    }

    pub async fn set_user_disabled(&self, id: UserId, disabled: bool) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "UPDATE users SET disabled = ?2 WHERE id = ?1",
                    params![id, disabled as i64],
                )?;
                Ok(n > 0)
            })
            .await
    }

    /// Apply an optional role and/or `disabled` change atomically, refusing any
    /// change that would leave the instance with no *enabled* admin. This mirrors
    /// the last-admin guard on [`Self::delete_user`], but also covers demotion
    /// (admin → member) and disabling the sole admin — either of which would
    /// otherwise silently lock every administrator out of the instance.
    pub async fn update_role_and_disabled(
        &self,
        id: UserId,
        role: Option<Role>,
        disabled: Option<bool>,
    ) -> anyhow::Result<UpdateUserOutcome> {
        // Nothing to change (and nothing to guard) when both fields are absent.
        if role.is_none() && disabled.is_none() {
            return Ok(UpdateUserOutcome::Updated);
        }
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let current: Option<(Role, bool)> = tx
                    .query_row(
                        "SELECT role, disabled FROM users WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get::<_, Role>(0)?, r.get::<_, i64>(1)? != 0)),
                    )
                    .optional()?;
                let Some((cur_role, cur_disabled)) = current else {
                    return Ok(UpdateUserOutcome::NotFound);
                };
                let new_role = role.unwrap_or(cur_role);
                let new_disabled = disabled.unwrap_or(cur_disabled);
                let was_enabled_admin = cur_role == Role::Admin && !cur_disabled;
                let will_be_enabled_admin = new_role == Role::Admin && !new_disabled;
                // Only a change that strips this user's *enabled admin* status can
                // reduce the pool; if it does and no other enabled admin remains,
                // refuse so the instance always keeps at least one.
                if was_enabled_admin && !will_be_enabled_admin {
                    let others: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM users \
                         WHERE role = 'admin' AND disabled = 0 AND id != ?1",
                        params![id],
                        |r| r.get(0),
                    )?;
                    if others == 0 {
                        return Ok(UpdateUserOutcome::LastAdmin);
                    }
                }
                tx.execute(
                    "UPDATE users SET role = ?2, disabled = ?3 WHERE id = ?1",
                    params![id, new_role, new_disabled as i64],
                )?;
                tx.commit()?;
                Ok(UpdateUserOutcome::Updated)
            })
            .await
    }

    pub async fn update_password(&self, id: UserId, password_hash: String) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "UPDATE users SET password_hash = ?2 WHERE id = ?1",
                    params![id, password_hash],
                )?;
                Ok(n > 0)
            })
            .await
    }

    /// Delete a user, refusing to remove the final *enabled* admin so the
    /// instance can never be locked out of its own administration. This mirrors
    /// the guard on [`Self::update_role_and_disabled`]: only an *enabled* admin
    /// counts toward the usable pool, so deleting a disabled admin is always
    /// allowed, and deleting an enabled admin is refused only when no other
    /// enabled admin remains. Counting all admins (including disabled ones)
    /// would let the sole enabled admin be deleted while a disabled admin
    /// lingers — a permanent lockout.
    pub async fn delete_user(&self, id: UserId) -> anyhow::Result<DeleteUserOutcome> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let target: Option<(Role, bool)> = tx
                    .query_row(
                        "SELECT role, disabled FROM users WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get::<_, Role>(0)?, r.get::<_, i64>(1)? != 0)),
                    )
                    .optional()?;
                let Some((role, disabled)) = target else {
                    return Ok(DeleteUserOutcome::NotFound);
                };
                // Only deleting an *enabled* admin can shrink the usable-admin
                // pool; if it does and no other enabled admin remains, refuse.
                if role == Role::Admin && !disabled {
                    let others: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM users \
                         WHERE role = 'admin' AND disabled = 0 AND id != ?1",
                        params![id],
                        |r| r.get(0),
                    )?;
                    if others == 0 {
                        return Ok(DeleteUserOutcome::LastAdmin);
                    }
                }
                tx.execute("DELETE FROM users WHERE id = ?1", params![id])?;
                tx.commit()?;
                Ok(DeleteUserOutcome::Deleted)
            })
            .await
    }
}

fn get_user<K: rusqlite::ToSql>(
    conn: &Connection,
    sql: &str,
    key: K,
) -> anyhow::Result<Option<UserRow>> {
    Ok(conn.query_row(sql, params![key], row_to_user).optional()?)
}

fn row_to_user(row: &rusqlite::Row) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        role: row.get(3)?,
        disabled: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        email: row.get(6)?,
    })
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// The credential-resolution query, factored out so it can run on either a
/// pooled reader or the writer depending on whether `last_used_at` is bumped.
fn query_session(
    conn: &Connection,
    token_hash: &str,
    now: i64,
) -> anyhow::Result<Option<SessionUser>> {
    Ok(conn
        .query_row(
            "SELECT u.id, u.username, u.role
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0",
            params![token_hash, now],
            |row| {
                Ok(SessionUser {
                    user_id: row.get(0)?,
                    username: row.get(1)?,
                    role: row.get(2)?,
                })
            },
        )
        .optional()?)
}

/// See [`query_session`] — same split, for API keys.
fn query_api_key(
    conn: &Connection,
    prefix: &str,
    key_hash: &str,
) -> anyhow::Result<Option<ApiKeyAuth>> {
    Ok(conn
        .query_row(
            "SELECT k.id, k.user_id, u.username, u.role, k.scope
             FROM api_keys k JOIN users u ON u.id = k.user_id
             WHERE k.prefix = ?1 AND k.key_hash = ?2 AND k.revoked = 0
               AND u.disabled = 0",
            params![prefix, key_hash],
            |row| {
                Ok(ApiKeyAuth {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    username: row.get(2)?,
                    role: row.get(3)?,
                    scope: row.get(4)?,
                })
            },
        )
        .optional()?)
}

impl Storage {
    pub async fn create_session(
        &self,
        token_hash: String,
        user_id: UserId,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        self.runs
            .write(move |conn| {
                let now = now_ms();
                conn.execute(
                    "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_used_at)
                     VALUES (?1, ?2, ?3, ?4, ?3)",
                    params![token_hash, user_id, now, expires_at],
                )?;
                Ok(())
            })
            .await
    }

    /// Resolve a session by its token hash. Returns `None` if it does not exist,
    /// has expired, or belongs to a disabled user.
    ///
    /// `bump_last_used` selects the connection this runs on, which is the whole
    /// point of the flag: refreshing `last_used_at` needs the exclusive writer,
    /// so an unthrottled bump would serialize *every* authenticated request
    /// against the single writer mutex. When `false` the lookup takes the
    /// reader pool instead and touches no rows. [`crate::auth::SessionAuthenticator`]
    /// decides, throttling the bump to at most once per session per minute.
    pub async fn lookup_session(
        &self,
        token_hash: String,
        bump_last_used: bool,
    ) -> anyhow::Result<Option<SessionUser>> {
        if !bump_last_used {
            return self
                .runs
                .read(move |conn| query_session(conn, &token_hash, now_ms()))
                .await;
        }
        self.runs
            .write(move |conn| {
                let now = now_ms();
                let found = query_session(conn, &token_hash, now)?;
                if found.is_some() {
                    conn.execute(
                        "UPDATE sessions SET last_used_at = ?2 WHERE token_hash = ?1",
                        params![token_hash, now],
                    )?;
                }
                Ok(found)
            })
            .await
    }

    pub async fn delete_session(&self, token_hash: String) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "DELETE FROM sessions WHERE token_hash = ?1",
                    params![token_hash],
                )?;
                Ok(n > 0)
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// API keys
// ---------------------------------------------------------------------------

impl Storage {
    pub async fn create_api_key(
        &self,
        user_id: UserId,
        name: Option<String>,
        prefix: String,
        key_hash: String,
        scope: Scope,
    ) -> anyhow::Result<ApiKeyInfo> {
        self.runs
            .write(move |conn| {
                let id: ApiKeyId = new_id();
                let created_at = now_ms();
                conn.execute(
                    "INSERT INTO api_keys
                        (id, user_id, name, prefix, key_hash, scope, created_at, last_used_at, revoked)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0)",
                    params![id, user_id, name, prefix, key_hash, scope, created_at],
                )?;
                Ok(ApiKeyInfo {
                    id,
                    user_id,
                    name,
                    prefix,
                    scope,
                    created_at,
                    last_used_at: None,
                    revoked: false,
                })
            })
            .await
    }

    /// Resolve an API key by prefix + hash. Returns `None` unless the key exists,
    /// is not revoked, and belongs to an enabled user.
    ///
    /// See [`Storage::lookup_session`] for what `bump_last_used` buys: `false`
    /// runs on the reader pool and writes nothing.
    pub async fn lookup_api_key(
        &self,
        prefix: String,
        key_hash: String,
        bump_last_used: bool,
    ) -> anyhow::Result<Option<ApiKeyAuth>> {
        if !bump_last_used {
            return self
                .runs
                .read(move |conn| query_api_key(conn, &prefix, &key_hash))
                .await;
        }
        self.runs
            .write(move |conn| {
                let now = now_ms();
                let found = query_api_key(conn, &prefix, &key_hash)?;
                if let Some(key) = &found {
                    conn.execute(
                        "UPDATE api_keys SET last_used_at = ?2 WHERE id = ?1",
                        params![key.id, now],
                    )?;
                }
                Ok(found)
            })
            .await
    }

    pub async fn list_api_keys(&self, user_id: UserId) -> anyhow::Result<Vec<ApiKeyInfo>> {
        self.runs
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, user_id, name, prefix, scope, created_at, last_used_at, revoked
                     FROM api_keys WHERE user_id = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![user_id], row_to_api_key)?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
    }

    pub async fn get_api_key(&self, id: ApiKeyId) -> anyhow::Result<Option<ApiKeyInfo>> {
        self.runs
            .read(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT id, user_id, name, prefix, scope, created_at, last_used_at, revoked
                         FROM api_keys WHERE id = ?1",
                        params![id],
                        row_to_api_key,
                    )
                    .optional()?)
            })
            .await
    }

    pub async fn revoke_api_key(&self, id: ApiKeyId) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n =
                    conn.execute("UPDATE api_keys SET revoked = 1 WHERE id = ?1", params![id])?;
                Ok(n > 0)
            })
            .await
    }
}

fn row_to_api_key(row: &rusqlite::Row) -> rusqlite::Result<ApiKeyInfo> {
    Ok(ApiKeyInfo {
        id: row.get(0)?,
        user_id: row.get(1)?,
        name: row.get(2)?,
        prefix: row.get(3)?,
        scope: row.get(4)?,
        created_at: row.get(5)?,
        last_used_at: row.get(6)?,
        revoked: row.get::<_, i64>(7)? != 0,
    })
}
