//! DTOs for the local-accounts/auth endpoints: `setup`, `login`, `logout`,
//! `me`, API keys, and user administration.

use chrono::{TimeZone, Utc};
use serde::Serialize;
use ts_rs::TS;

use crate::auth::{IdentitySource, Scope};
use crate::domain::{ApiKeyId, Role, SsoKind, UserId};
use crate::storage::{ApiKeyInfo, UserIdentityRow, UserRow};

/// One linked SSO identity, safe to expose (no tokens). `provider` is the
/// namespaced key (`oidc:google`), which the UI splits for display.
#[derive(Debug, Clone, Serialize, TS)]
pub struct UserIdentityView {
    pub provider: String,
    pub kind: SsoKind,
    pub subject: String,
    pub email: Option<String>,
    /// RFC3339, or `None` if never used since linking.
    pub last_login_at: Option<String>,
}

impl From<&UserIdentityRow> for UserIdentityView {
    fn from(row: &UserIdentityRow) -> Self {
        UserIdentityView {
            provider: row.provider.clone(),
            kind: row.kind,
            subject: row.subject.clone(),
            email: row.email.clone(),
            last_login_at: row.last_login_at.map(rfc3339),
        }
    }
}

/// One account, safe to expose (no password hash).
#[derive(Debug, Clone, Serialize, TS)]
pub struct UserView {
    pub id: UserId,
    pub username: String,
    pub role: Role,
    pub disabled: bool,
    /// RFC3339.
    pub created_at: String,
    /// Only set for SSO-provisioned accounts.
    pub email: Option<String>,
    /// Whether a password login is possible (false for SSO-only accounts).
    pub has_password: bool,
    /// Linked SSO identities (empty for local accounts).
    pub identities: Vec<UserIdentityView>,
    /// The provider key whose IdP controls this user's role, when any SSO
    /// identity is linked — its role is re-synced on each SSO login and the
    /// admin UI should reflect that rather than offer a manual override.
    pub role_managed_by: Option<String>,
}

impl UserView {
    /// Project a user row and its identity links onto the wire shape.
    pub fn from_row(user: &UserRow, identities: &[UserIdentityRow]) -> Self {
        UserView {
            id: user.id.clone(),
            username: user.username.clone(),
            role: user.role,
            disabled: user.disabled,
            created_at: rfc3339(user.created_at),
            email: user.email.clone(),
            has_password: !user.password_hash.is_empty(),
            identities: identities.iter().map(UserIdentityView::from).collect(),
            role_managed_by: identities.first().map(|i| i.provider.clone()),
        }
    }
}

impl From<&UserRow> for UserView {
    /// A user with no identity links (local accounts, and the single-user
    /// responses that fetch links separately when relevant).
    fn from(user: &UserRow) -> Self {
        UserView::from_row(user, &[])
    }
}

/// `GET /users` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct UserListResponse {
    pub users: Vec<UserView>,
}

/// One API key's metadata (never the secret or its hash).
#[derive(Debug, Clone, Serialize, TS)]
pub struct ApiKeyView {
    pub id: ApiKeyId,
    pub name: Option<String>,
    pub prefix: String,
    pub scope: Scope,
    /// RFC3339.
    pub created_at: String,
    /// RFC3339, or `None` if the key has never been used.
    pub last_used_at: Option<String>,
    pub revoked: bool,
}

impl From<&ApiKeyInfo> for ApiKeyView {
    fn from(info: &ApiKeyInfo) -> Self {
        ApiKeyView {
            id: info.id.clone(),
            name: info.name.clone(),
            prefix: info.prefix.clone(),
            scope: info.scope,
            created_at: rfc3339(info.created_at),
            last_used_at: info.last_used_at.map(rfc3339),
            revoked: info.revoked,
        }
    }
}

/// `GET /apikeys` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ApiKeyListResponse {
    pub keys: Vec<ApiKeyView>,
}

/// `POST /apikeys` response: a freshly minted key's metadata plus its secret,
/// shown exactly once (never again — only the hash is stored). `#[serde(flatten)]`
/// reproduces the pre-DTO shape exactly: one flat object with `key` alongside
/// every [`ApiKeyView`] field, not a nested `view` object.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ApiKeyCreatedResponse {
    pub key: String,
    #[serde(flatten)]
    pub view: ApiKeyView,
}

/// The compact user shape embedded in [`MeResponse`] — distinct from
/// [`UserView`], which also carries `disabled`/`created_at`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MeUser {
    pub id: UserId,
    pub username: String,
    pub role: Role,
    /// Linked SSO identities so Settings can show sign-in methods.
    pub identities: Vec<UserIdentityView>,
    /// The provider key managing this user's role (see [`UserView`]).
    pub role_managed_by: Option<String>,
}

/// `GET /auth/me` response. Every field is always present on the wire (never
/// omitted); `user` and `scope` serialize as `null`, not absent, when the
/// caller is anonymous.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MeResponse {
    pub authenticated: bool,
    pub user: Option<MeUser>,
    pub source: IdentitySource,
    pub scope: Option<Scope>,
}

/// `POST /auth/setup` and `POST /auth/login` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct AuthSessionResponse {
    pub token: String,
    pub user: UserView,
}

/// `POST /auth/logout` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct OkResponse {
    pub ok: bool,
}

/// Epoch milliseconds -> RFC3339, replicating the conversion `user_json`/
/// `apikey_json` used before this module existed (down to the empty-string
/// fallback on an out-of-range timestamp — practically unreachable, since
/// every caller here supplies a value fresh from `now_ms()` or a
/// `created_at`/`last_used_at` DB column).
fn rfc3339(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_user_row() -> UserRow {
        UserRow {
            id: UserId::new("usr_1"),
            username: "root".to_string(),
            password_hash: "$argon2id$secret".to_string(),
            role: Role::Admin,
            disabled: false,
            created_at: 1_735_689_600_000, // 2025-01-01T00:00:00Z
            email: None,
        }
    }

    fn sample_api_key_info() -> ApiKeyInfo {
        ApiKeyInfo {
            id: ApiKeyId::new("key_1"),
            user_id: UserId::new("usr_1"),
            name: Some("ci".to_string()),
            prefix: "domarinn_deadbee".to_string(),
            scope: Scope::Write,
            created_at: 1_735_689_600_000,
            last_used_at: None,
            revoked: false,
        }
    }

    fn sample_identity_row() -> UserIdentityRow {
        UserIdentityRow {
            id: "idn_1".to_string(),
            user_id: UserId::new("usr_1"),
            provider: "oidc:google".to_string(),
            kind: SsoKind::Oidc,
            subject: "sub-123".to_string(),
            email: Some("root@example.com".to_string()),
            display_name: Some("Root".to_string()),
            created_at: 1_735_689_600_000,
            last_login_at: Some(1_735_776_000_000), // 2025-01-02T00:00:00Z
        }
    }

    #[test]
    fn user_view_matches_todays_wire_shape() {
        let dto = UserView::from(&sample_user_row());
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "id": "usr_1",
                "username": "root",
                "role": "admin",
                "disabled": false,
                "created_at": "2025-01-01T00:00:00+00:00",
                "email": null,
                "has_password": true,
                "identities": [],
                "role_managed_by": null,
            })
        );
    }

    #[test]
    fn user_view_surfaces_linked_sso_identities() {
        let mut row = sample_user_row();
        row.password_hash = String::new(); // SSO-only
        row.email = Some("root@example.com".to_string());
        let dto = UserView::from_row(&row, &[sample_identity_row()]);
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["has_password"], false);
        assert_eq!(v["email"], "root@example.com");
        assert_eq!(v["role_managed_by"], "oidc:google");
        assert_eq!(v["identities"][0]["provider"], "oidc:google");
        assert_eq!(v["identities"][0]["kind"], "oidc");
        assert_eq!(
            v["identities"][0]["last_login_at"],
            "2025-01-02T00:00:00+00:00"
        );
        // The subject is exposed for admin display but never a token.
        assert_eq!(v["identities"][0]["subject"], "sub-123");
    }

    #[test]
    fn user_view_never_leaks_the_password_hash() {
        let v = serde_json::to_value(UserView::from(&sample_user_row())).unwrap();
        assert!(v.get("password_hash").is_none());
        assert_eq!(v.as_object().unwrap().len(), 9);
    }

    #[test]
    fn user_list_response_matches_todays_wire_shape() {
        let dto = UserListResponse {
            users: vec![UserView::from(&sample_user_row())],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "users": [
                    {
                        "id": "usr_1",
                        "username": "root",
                        "role": "admin",
                        "disabled": false,
                        "created_at": "2025-01-01T00:00:00+00:00",
                        "email": null,
                        "has_password": true,
                        "identities": [],
                        "role_managed_by": null,
                    }
                ]
            })
        );
    }

    #[test]
    fn api_key_view_matches_todays_wire_shape() {
        let dto = ApiKeyView::from(&sample_api_key_info());
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "id": "key_1",
                "name": "ci",
                "prefix": "domarinn_deadbee",
                "scope": "write",
                "created_at": "2025-01-01T00:00:00+00:00",
                "last_used_at": null,
                "revoked": false,
            })
        );
    }

    #[test]
    fn api_key_view_never_leaks_user_id() {
        let v = serde_json::to_value(ApiKeyView::from(&sample_api_key_info())).unwrap();
        assert!(v.get("user_id").is_none());
    }

    #[test]
    fn api_key_view_reports_last_used_at_when_present() {
        let mut info = sample_api_key_info();
        info.last_used_at = Some(1_735_776_000_000); // 2025-01-02T00:00:00Z
        let v = serde_json::to_value(ApiKeyView::from(&info)).unwrap();
        assert_eq!(v["last_used_at"], "2025-01-02T00:00:00+00:00");
    }

    #[test]
    fn api_key_list_response_matches_todays_wire_shape() {
        let dto = ApiKeyListResponse {
            keys: vec![ApiKeyView::from(&sample_api_key_info())],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "keys": [
                    {
                        "id": "key_1",
                        "name": "ci",
                        "prefix": "domarinn_deadbee",
                        "scope": "write",
                        "created_at": "2025-01-01T00:00:00+00:00",
                        "last_used_at": null,
                        "revoked": false,
                    }
                ]
            })
        );
    }

    #[test]
    fn api_key_created_response_flattens_to_the_same_flat_object_plus_key() {
        // Today's shape: `apikey_json(&info)` with a `"key"` field inserted
        // into the same map — one flat object, not a nested `view`.
        let dto = ApiKeyCreatedResponse {
            key: "domarinn_deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            view: ApiKeyView::from(&sample_api_key_info()),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "key": "domarinn_deadbeefdeadbeefdeadbeefdeadbeef",
                "id": "key_1",
                "name": "ci",
                "prefix": "domarinn_deadbee",
                "scope": "write",
                "created_at": "2025-01-01T00:00:00+00:00",
                "last_used_at": null,
                "revoked": false,
            })
        );
    }

    #[test]
    fn me_response_matches_todays_wire_shape_when_authenticated() {
        let dto = MeResponse {
            authenticated: true,
            user: Some(MeUser {
                id: UserId::new("usr_1"),
                username: "root".to_string(),
                role: Role::Admin,
                identities: Vec::new(),
                role_managed_by: None,
            }),
            source: IdentitySource::Session,
            scope: Some(Scope::Admin),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "authenticated": true,
                "user": {
                    "id": "usr_1",
                    "username": "root",
                    "role": "admin",
                    "identities": [],
                    "role_managed_by": null,
                },
                "source": "session",
                "scope": "admin",
            })
        );
    }

    #[test]
    fn me_response_matches_todays_wire_shape_when_anonymous() {
        // Anonymous: every field still present, `user`/`scope` are explicit
        // JSON null (json! never omitted them).
        let dto = MeResponse {
            authenticated: false,
            user: None,
            source: IdentitySource::Anonymous,
            scope: None,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(
            v,
            json!({
                "authenticated": false,
                "user": null,
                "source": "anonymous",
                "scope": null,
            })
        );
        for key in ["user", "scope"] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
    }

    #[test]
    fn identity_source_serializes_to_todays_wire_strings() {
        assert_eq!(
            serde_json::to_value(IdentitySource::Anonymous).unwrap(),
            json!("anonymous")
        );
        assert_eq!(
            serde_json::to_value(IdentitySource::Static).unwrap(),
            json!("static")
        );
        assert_eq!(
            serde_json::to_value(IdentitySource::ApiKey).unwrap(),
            json!("apikey")
        );
        assert_eq!(
            serde_json::to_value(IdentitySource::Session).unwrap(),
            json!("session")
        );
    }

    #[test]
    fn auth_session_response_matches_todays_wire_shape() {
        let dto = AuthSessionResponse {
            token: "mses_deadbeef".to_string(),
            user: UserView::from(&sample_user_row()),
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "token": "mses_deadbeef",
                "user": {
                    "id": "usr_1",
                    "username": "root",
                    "role": "admin",
                    "disabled": false,
                    "created_at": "2025-01-01T00:00:00+00:00",
                    "email": null,
                    "has_password": true,
                    "identities": [],
                    "role_managed_by": null,
                },
            })
        );
    }

    #[test]
    fn ok_response_matches_todays_wire_shape() {
        let dto = OkResponse { ok: true };
        assert_eq!(serde_json::to_value(&dto).unwrap(), json!({ "ok": true }));
    }
}
