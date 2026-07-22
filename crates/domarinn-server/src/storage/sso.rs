//! SSO identity links, in-flight login transactions, and the SAML replay
//! cache (migration 4).
//!
//! Identities match strictly on `(provider, subject)` — never by email, which
//! an IdP may let users change — so an attacker who controls an email address
//! at one provider can never take over an account provisioned by another.
//! Providers are stored namespaced (`oidc:google`, `saml:okta`) so OIDC and
//! SAML provider names can never collide.

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use super::{now_ms, Storage, UserRow};
use crate::domain::{Role, SsoKind, UserId};

/// A linked IdP identity.
#[derive(Debug, Clone)]
pub struct UserIdentityRow {
    pub id: String,
    pub user_id: UserId,
    /// Namespaced provider key, e.g. `oidc:google`.
    pub provider: String,
    pub kind: SsoKind,
    /// OIDC `sub` / SAML `NameID`.
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
}

/// The identity payload stored when JIT-provisioning a new SSO user.
#[derive(Debug, Clone)]
pub struct NewIdentity {
    pub provider: String,
    pub kind: SsoKind,
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

/// An in-flight login transaction: OIDC state/nonce/PKCE or a SAML request
/// id, plus where to send the browser afterwards. Keyed (hashed) by a random
/// id that doubles as the OIDC `state` / SAML `RelayState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginTxn {
    pub kind: SsoKind,
    pub provider: String,
    pub nonce: Option<String>,
    pub pkce_verifier: Option<String>,
    pub request_id: Option<String>,
    pub return_to: Option<String>,
}

/// How long an in-flight login transaction stays valid.
pub const LOGIN_TXN_TTL_MS: i64 = 10 * 60 * 1000;

/// The expiry timestamp (epoch ms) for a login transaction minted now.
pub fn login_txn_expiry() -> i64 {
    now_ms() + LOGIN_TXN_TTL_MS
}

impl Storage {
    /// Persist an in-flight login transaction (and opportunistically purge
    /// expired ones so the table can never grow without bound).
    pub async fn create_login_txn(
        &self,
        id_hash: String,
        txn: LoginTxn,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        self.runs
            .write(move |conn| {
                let now = now_ms();
                conn.execute(
                    "DELETE FROM login_transactions WHERE expires_at <= ?1",
                    params![now],
                )?;
                conn.execute(
                    "INSERT INTO login_transactions
                        (id_hash, kind, provider, nonce, pkce_verifier, request_id, return_to,
                         created_at, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        id_hash,
                        txn.kind,
                        txn.provider,
                        txn.nonce,
                        txn.pkce_verifier,
                        txn.request_id,
                        txn.return_to,
                        now,
                        expires_at
                    ],
                )?;
                Ok(())
            })
            .await
    }

    /// Consume a login transaction: single-use by construction
    /// (`DELETE ... RETURNING`), and expired rows return `None` even when the
    /// GC has not swept them yet.
    pub async fn take_login_txn(&self, id_hash: String) -> anyhow::Result<Option<LoginTxn>> {
        self.runs
            .write(move |conn| {
                let row = conn
                    .query_row(
                        "DELETE FROM login_transactions WHERE id_hash = ?1
                         RETURNING kind, provider, nonce, pkce_verifier, request_id, return_to,
                                   expires_at",
                        params![id_hash],
                        |row| {
                            Ok((
                                LoginTxn {
                                    kind: row.get(0)?,
                                    provider: row.get(1)?,
                                    nonce: row.get(2)?,
                                    pkce_verifier: row.get(3)?,
                                    request_id: row.get(4)?,
                                    return_to: row.get(5)?,
                                },
                                row.get::<_, i64>(6)?,
                            ))
                        },
                    )
                    .optional()?;
                Ok(row.and_then(|(txn, expires_at)| (expires_at > now_ms()).then_some(txn)))
            })
            .await
    }

    /// Resolve an identity (and its user) by the namespaced provider key and
    /// IdP subject.
    pub async fn get_identity_user(
        &self,
        provider: String,
        subject: String,
    ) -> anyhow::Result<Option<(UserIdentityRow, UserRow)>> {
        self.runs
            .read(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT i.id, i.user_id, i.provider, i.kind, i.subject, i.email,
                                i.display_name, i.created_at, i.last_login_at,
                                u.id, u.username, u.password_hash, u.role, u.disabled,
                                u.created_at, u.email
                         FROM user_identities i JOIN users u ON u.id = i.user_id
                         WHERE i.provider = ?1 AND i.subject = ?2",
                        params![provider, subject],
                        |row| {
                            Ok((
                                row_to_identity(row, 0)?,
                                UserRow {
                                    id: row.get(9)?,
                                    username: row.get(10)?,
                                    password_hash: row.get(11)?,
                                    role: row.get(12)?,
                                    disabled: row.get::<_, i64>(13)? != 0,
                                    created_at: row.get(14)?,
                                    email: row.get(15)?,
                                },
                            ))
                        },
                    )
                    .optional()?)
            })
            .await
    }

    /// JIT-provision a new SSO user with its identity link in one transaction.
    /// The username starts from `username_base` and steps through `-2`..`-20`
    /// suffixes (then a random one) on collision. The stored `password_hash`
    /// is the `''` sentinel: SSO-only users have no password login.
    pub async fn create_sso_user(
        &self,
        username_base: String,
        email: Option<String>,
        role: Role,
        identity: NewIdentity,
    ) -> anyhow::Result<UserRow> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let username = {
                    let taken = |tx: &rusqlite::Transaction, name: &str| -> anyhow::Result<bool> {
                        Ok(tx
                            .query_row(
                                "SELECT 1 FROM users WHERE username = ?1",
                                params![name],
                                |_| Ok(()),
                            )
                            .optional()?
                            .is_some())
                    };
                    let mut chosen = None;
                    if !taken(&tx, &username_base)? {
                        chosen = Some(username_base.clone());
                    } else {
                        for n in 2..=20 {
                            let candidate = format!("{username_base}-{n}");
                            if !taken(&tx, &candidate)? {
                                chosen = Some(candidate);
                                break;
                            }
                        }
                    }
                    chosen.unwrap_or_else(|| {
                        format!(
                            "{username_base}-{}",
                            ulid::Ulid::generate().to_string().to_lowercase()
                        )
                    })
                };

                let user_id: UserId = super::auth::new_id();
                let identity_id: String = ulid::Ulid::generate().to_string();
                let now = now_ms();
                tx.execute(
                    "INSERT INTO users (id, username, password_hash, role, disabled, created_at, email)
                     VALUES (?1, ?2, '', ?3, 0, ?4, ?5)",
                    params![user_id, username, role, now, email],
                )?;
                tx.execute(
                    "INSERT INTO user_identities
                        (id, user_id, provider, kind, subject, email, display_name,
                         created_at, last_login_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                    params![
                        identity_id,
                        user_id,
                        identity.provider,
                        identity.kind,
                        identity.subject,
                        identity.email,
                        identity.display_name,
                        now
                    ],
                )?;
                tx.commit()?;
                Ok(UserRow {
                    id: user_id,
                    username,
                    password_hash: String::new(),
                    role,
                    disabled: false,
                    created_at: now,
                    email,
                })
            })
            .await
    }

    /// Refresh an identity's email/display-name snapshot and `last_login_at`
    /// on a successful SSO login.
    pub async fn touch_identity(
        &self,
        id: String,
        email: Option<String>,
        display_name: Option<String>,
    ) -> anyhow::Result<()> {
        self.runs
            .write(move |conn| {
                conn.execute(
                    "UPDATE user_identities
                     SET email = ?2, display_name = ?3, last_login_at = ?4
                     WHERE id = ?1",
                    params![id, email, display_name, now_ms()],
                )?;
                Ok(())
            })
            .await
    }

    /// Every identity link, for the admin user list (grouped by caller).
    pub async fn list_all_identities(&self) -> anyhow::Result<Vec<UserIdentityRow>> {
        self.runs
            .read(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, user_id, provider, kind, subject, email, display_name,
                            created_at, last_login_at
                     FROM user_identities ORDER BY created_at ASC",
                )?;
                let rows = stmt.query_map([], |row| row_to_identity(row, 0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
    }

    /// The identity links of a single user (the `me` endpoint and single-user
    /// admin responses).
    pub async fn list_identities_for_user(
        &self,
        user_id: UserId,
    ) -> anyhow::Result<Vec<UserIdentityRow>> {
        self.runs
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, user_id, provider, kind, subject, email, display_name,
                            created_at, last_login_at
                     FROM user_identities WHERE user_id = ?1 ORDER BY created_at ASC",
                )?;
                let rows = stmt.query_map(params![user_id], |row| row_to_identity(row, 0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await
    }

    /// Record a consumed SAML assertion id. Returns `false` when the id was
    /// already present — a replay — and purges expired entries in passing.
    pub async fn saml_mark_assertion(
        &self,
        assertion_id: String,
        provider: String,
        not_on_or_after: i64,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                conn.execute(
                    "DELETE FROM saml_consumed_assertions WHERE not_on_or_after <= ?1",
                    params![now_ms()],
                )?;
                let inserted = conn.execute(
                    "INSERT OR IGNORE INTO saml_consumed_assertions
                        (assertion_id, provider, not_on_or_after)
                     VALUES (?1, ?2, ?3)",
                    params![assertion_id, provider, not_on_or_after],
                )?;
                Ok(inserted > 0)
            })
            .await
    }

    /// Sweep expired login transactions and replay-cache entries. Called by
    /// the hourly retention task; inserts also purge opportunistically.
    pub async fn sso_gc(&self) -> anyhow::Result<()> {
        self.runs
            .write(|conn| {
                let now = now_ms();
                conn.execute(
                    "DELETE FROM login_transactions WHERE expires_at <= ?1",
                    params![now],
                )?;
                conn.execute(
                    "DELETE FROM saml_consumed_assertions WHERE not_on_or_after <= ?1",
                    params![now],
                )?;
                Ok(())
            })
            .await
    }
}

fn row_to_identity(row: &rusqlite::Row, offset: usize) -> rusqlite::Result<UserIdentityRow> {
    Ok(UserIdentityRow {
        id: row.get(offset)?,
        user_id: row.get(offset + 1)?,
        provider: row.get(offset + 2)?,
        kind: row.get(offset + 3)?,
        subject: row.get(offset + 4)?,
        email: row.get(offset + 5)?,
        display_name: row.get(offset + 6)?,
        created_at: row.get(offset + 7)?,
        last_login_at: row.get(offset + 8)?,
    })
}
