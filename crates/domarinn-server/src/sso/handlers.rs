//! HTTP handlers for the SSO login flows.
//!
//! All of these are unauthenticated by design — they ARE the way in. Flow
//! failures never render JSON to the browser mid-redirect; they 303 back to
//! `/login?sso_error=<code>&provider=<name>` with details only in the log.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::auth;
use crate::domain::SsoKind;
use crate::routes::not_found;
use crate::sso::SsoError;
use crate::storage::{login_txn_expiry, LoginTxn, LOGIN_TXN_TTL_MS};
use crate::AppState;

/// Short-lived cookie binding an in-flight OIDC transaction to the browser
/// that started it (login-CSRF defense). `SameSite=Lax` still sends it on
/// the IdP's top-level redirect back to the callback. SAML deliberately has
/// no such binding: the ACS is a cross-site POST where a Lax cookie is
/// absent; unsolicited responses are rejected via `InResponseTo` instead.
pub(crate) const TXN_COOKIE: &str = "domarinn_txn";

/// Binds a SAML login to the browser that initiated it. Unlike the OIDC
/// `domarinn_txn` cookie (which can be `SameSite=Lax` — the callback is a
/// top-level GET), this must ride the cross-site ACS POST, so over HTTPS it
/// is `SameSite=None; Secure`.
#[cfg(feature = "saml")]
pub(crate) const SAML_TXN_COOKIE: &str = "domarinn_saml_txn";

// ---------------------------------------------------------------------------
// OIDC
// ---------------------------------------------------------------------------

/// IdPs append extra query params freely (Keycloak's `session_state`,
/// Google's `authuser`), so these structs must NOT deny unknown fields.
#[derive(Debug, Deserialize)]
pub(crate) struct StartQuery {
    return_to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OidcCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// `GET /auth/oidc/{provider}/start` — mint a login transaction and bounce
/// the browser to the IdP.
pub(crate) async fn oidc_start(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<StartQuery>,
) -> Response {
    let Some(oidc) = state.sso.oidc(&provider) else {
        return not_found("sso provider").into_response();
    };
    let Some(public_url) = state.public_url.as_deref() else {
        // Unreachable: the registry refuses providers without a public URL.
        return login_error(
            &provider,
            &SsoError::Internal(anyhow::anyhow!("no public URL")),
        );
    };

    let txn_id = auth::generate_txn_id();
    let begin = match oidc.begin(&oidc.redirect_uri(public_url), &txn_id).await {
        Ok(begin) => begin,
        Err(e) => return login_error(&provider, &e),
    };

    let txn = LoginTxn {
        kind: SsoKind::Oidc,
        provider: provider.clone(),
        nonce: Some(begin.nonce),
        pkce_verifier: Some(begin.pkce_verifier),
        request_id: None,
        return_to: validate_return_to(query.return_to.as_deref()),
    };
    if let Err(e) = state
        .storage
        .create_login_txn(auth::token_hash(&txn_id), txn, login_txn_expiry())
        .await
    {
        return login_error(&provider, &SsoError::Internal(e));
    }

    let mut response = Redirect::to(&begin.auth_url).into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, txn_cookie(&txn_id, state.cookie_secure));
    response
}

/// `GET /auth/oidc/{provider}/callback` — consume the transaction, exchange
/// the code, provision the user, set the session cookie, and send the
/// browser home (or back to its deep link).
pub(crate) async fn oidc_callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<OidcCallbackQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(oidc) = state.sso.oidc(&provider) else {
        return not_found("sso provider").into_response();
    };

    if let Some(error) = &query.error {
        tracing::warn!(
            provider = %provider,
            error = %error,
            description = query.error_description.as_deref().unwrap_or(""),
            "IdP returned an authorization error"
        );
        return login_error(&provider, &SsoError::AccessDenied);
    }
    let (Some(code), Some(cb_state)) = (&query.code, &query.state) else {
        return login_error(&provider, &SsoError::InvalidState);
    };

    // The browser must present the txn cookie matching `state` — a callback
    // pasted into a victim's browser (login CSRF) has no such cookie.
    if cookie_value(&headers, TXN_COOKIE).as_deref() != Some(cb_state.as_str()) {
        return login_error(&provider, &SsoError::InvalidState);
    }

    let txn = match state
        .storage
        .take_login_txn(auth::token_hash(cb_state))
        .await
    {
        Ok(Some(txn)) if txn.kind == SsoKind::Oidc && txn.provider == provider => txn,
        Ok(_) => return login_error(&provider, &SsoError::InvalidState),
        Err(e) => return login_error(&provider, &SsoError::Internal(e)),
    };

    let public_url = state.public_url.as_deref().unwrap_or_default();
    let asserted = match oidc
        .complete(code, &txn, &oidc.redirect_uri(public_url))
        .await
    {
        Ok(asserted) => asserted,
        Err(e) => return login_error(&provider, &e),
    };

    finish_login(
        &state,
        &provider,
        &format!("oidc:{provider}"),
        SsoKind::Oidc,
        &asserted,
        &oidc.cfg.mapping,
        txn.return_to.as_deref(),
        true,
    )
    .await
}

// ---------------------------------------------------------------------------
// SAML
// ---------------------------------------------------------------------------

/// The HTTP-POST binding form the IdP submits to the ACS. Field names are
/// fixed by the SAML spec.
#[cfg(feature = "saml")]
#[derive(Debug, Deserialize)]
pub(crate) struct AcsForm {
    #[serde(rename = "SAMLResponse")]
    saml_response: String,
    #[serde(rename = "RelayState")]
    relay_state: Option<String>,
}

/// `GET /auth/saml/{provider}/start` — mint a login transaction (its id is
/// the RelayState) and bounce the browser to the IdP with a deflated
/// `SAMLRequest`.
///
/// A `domarinn_saml_txn` cookie binds this login to the browser that started
/// it. Without it, `InResponseTo` alone stops *unsolicited* and *replayed*
/// responses but not a login-CSRF / session-swap: an attacker who completes
/// their own SP-initiated login can force the resulting (validly signed)
/// response into a victim's browser and log the victim in as the attacker.
/// The cookie closes that hole — the ACS requires it to match the RelayState.
/// Over HTTPS it is `SameSite=None; Secure` so it rides the cross-site ACS
/// POST; on plain HTTP it degrades to `Lax` (SAML deployments should use TLS).
#[cfg(feature = "saml")]
pub(crate) async fn saml_start(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<StartQuery>,
) -> Response {
    let Some(saml) = state.sso.saml(&provider) else {
        return not_found("sso provider").into_response();
    };

    let txn_id = auth::generate_txn_id();
    let begin = match saml.begin(&txn_id) {
        Ok(begin) => begin,
        Err(e) => return login_error(&provider, &e),
    };

    let txn = LoginTxn {
        kind: SsoKind::Saml,
        provider: provider.clone(),
        nonce: None,
        pkce_verifier: None,
        request_id: Some(begin.request_id),
        return_to: validate_return_to(query.return_to.as_deref()),
    };
    if let Err(e) = state
        .storage
        .create_login_txn(auth::token_hash(&txn_id), txn, login_txn_expiry())
        .await
    {
        return login_error(&provider, &SsoError::Internal(e));
    }

    let mut response = Redirect::to(&begin.redirect_url).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        saml_txn_cookie(&txn_id, state.cookie_secure),
    );
    response
}

/// `POST /auth/saml/{provider}/acs` — verify the response, enforce
/// single-use via the replay cache, provision, and redirect.
#[cfg(feature = "saml")]
pub(crate) async fn saml_acs(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<AcsForm>,
) -> Response {
    let Some(saml) = state.sso.saml(&provider) else {
        return not_found("sso provider").into_response();
    };

    // Resolve the transaction (SP-initiated) or fall through to the
    // IdP-initiated path when that is explicitly allowed.
    let (expected_request_id, return_to) = match &form.relay_state {
        Some(relay) if !relay.is_empty() => {
            // Login-CSRF / session-swap defense: the browser must present the
            // txn cookie set at `saml_start` matching this RelayState. A
            // response replayed into a victim's browser lacks it.
            if cookie_value(&headers, SAML_TXN_COOKIE).as_deref() != Some(relay.as_str()) {
                return login_error(&provider, &SsoError::InvalidState);
            }
            match state.storage.take_login_txn(auth::token_hash(relay)).await {
                Ok(Some(txn)) if txn.kind == SsoKind::Saml && txn.provider == provider => {
                    match txn.request_id {
                        Some(id) => (Some(id), txn.return_to),
                        None => return login_error(&provider, &SsoError::InvalidState),
                    }
                }
                Ok(_) => return login_error(&provider, &SsoError::InvalidState),
                Err(e) => return login_error(&provider, &SsoError::Internal(e)),
            }
        }
        _ if saml.allow_idp_initiated => (None, None),
        _ => return login_error(&provider, &SsoError::InvalidState),
    };

    let login = match saml.complete(&form.saml_response, expected_request_id.as_deref()) {
        Ok(login) => login,
        Err(e) => return login_error(&provider, &e),
    };

    // Replay cache: each assertion id is accepted exactly once.
    match state
        .storage
        .saml_mark_assertion(
            login.assertion_id.clone(),
            format!("saml:{provider}"),
            login.replay_expiry_ms,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => return login_error(&provider, &SsoError::Replayed),
        Err(e) => return login_error(&provider, &SsoError::Internal(e)),
    }

    finish_login(
        &state,
        &provider,
        &format!("saml:{provider}"),
        SsoKind::Saml,
        &login.asserted,
        &saml.mapping,
        return_to.as_deref(),
        false,
    )
    .await
}

/// `GET /auth/saml/{provider}/metadata` — the SP metadata XML IdPs import.
#[cfg(feature = "saml")]
pub(crate) async fn saml_metadata(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Response {
    let Some(saml) = state.sso.saml(&provider) else {
        return not_found("sso provider").into_response();
    };
    match saml.sp_metadata_xml() {
        Ok(xml) => (
            [(header::CONTENT_TYPE, "application/samlmetadata+xml")],
            xml,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(provider = %provider, error = %e, "SP metadata generation failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "error": "internal server error" })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Shared flow tail + helpers
// ---------------------------------------------------------------------------

/// Provision the user, mint the session, and 303 the browser to its
/// destination with the session cookie (clearing the txn cookie when asked).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finish_login(
    state: &AppState,
    provider: &str,
    provider_key: &str,
    kind: SsoKind,
    asserted: &crate::sso::AssertedIdentity,
    mapping: &crate::sso::RoleMapping,
    return_to: Option<&str>,
    clear_txn_cookie: bool,
) -> Response {
    let user = match crate::sso::provision::provision(
        &state.storage,
        provider_key,
        kind,
        asserted,
        mapping,
    )
    .await
    {
        Ok(user) => user,
        Err(e) => return login_error(provider, &e),
    };

    let token = match crate::accounts::issue_session(state, &user.id).await {
        Ok(token) => token,
        Err(_) => {
            return login_error(
                provider,
                &SsoError::Internal(anyhow::anyhow!("failed to mint session")),
            )
        }
    };
    tracing::info!(
        provider = provider_key,
        username = %user.username,
        role = %user.role,
        "sso login"
    );

    let destination = return_to.unwrap_or("/");
    let mut response = Redirect::to(destination).into_response();
    let headers = response.headers_mut();
    headers.append(
        header::SET_COOKIE,
        auth::session_cookie(&token, state.cookie_secure),
    );
    if clear_txn_cookie {
        headers.append(
            header::SET_COOKIE,
            clear_txn_cookie_value(state.cookie_secure),
        );
    }
    response
}

/// 303 to the login page with a machine error code; details only in logs.
pub(crate) fn login_error(provider: &str, error: &SsoError) -> Response {
    match error {
        SsoError::Provider(_) | SsoError::Internal(_) => {
            tracing::error!(provider = %provider, error = %error, "sso login failed")
        }
        other => tracing::warn!(provider = %provider, code = other.code(), "sso login rejected"),
    }
    let location = format!(
        "/login?sso_error={}&provider={}",
        error.code(),
        urlencode(provider)
    );
    Redirect::to(&location).into_response()
}

/// Only same-origin absolute paths survive; anything else falls back to the
/// app root. The path must start with a single `/` not followed by another
/// `/` or a `\` (browsers normalize a leading `\` to `/`, so `/\evil.com` —
/// or `//evil` — resolves to `https://evil.com`, an open redirect), and must
/// contain no control characters or backslashes anywhere.
pub(crate) fn validate_return_to(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let mut chars = raw.chars();
    if chars.next() != Some('/') {
        return None;
    }
    if matches!(chars.next(), Some('/') | Some('\\')) {
        return None;
    }
    if raw.contains('\\') || raw.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(raw.to_string())
}

pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(raw) = value.to_str() else { continue };
        for parsed in cookie::Cookie::split_parse(raw).flatten() {
            if parsed.name() == name && !parsed.value().is_empty() {
                return Some(parsed.value().to_string());
            }
        }
    }
    None
}

fn txn_cookie(value: &str, secure: bool) -> HeaderValue {
    build_txn_cookie(value, LOGIN_TXN_TTL_MS / 1000, secure)
}

fn clear_txn_cookie_value(secure: bool) -> HeaderValue {
    build_txn_cookie("", 0, secure)
}

fn build_txn_cookie(value: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    let cookie = cookie::Cookie::build((TXN_COOKIE, value))
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .path("/api/v1/auth")
        .max_age(cookie::time::Duration::seconds(max_age_secs))
        .secure(secure)
        .build();
    HeaderValue::from_str(&cookie.to_string()).expect("txn cookie is always valid ASCII")
}

/// The SAML txn-binding cookie. `SameSite=None; Secure` over HTTPS so it is
/// sent on the cross-site ACS POST; `Lax` on plain HTTP (browsers reject
/// `SameSite=None` without `Secure`).
#[cfg(feature = "saml")]
fn saml_txn_cookie(value: &str, secure: bool) -> HeaderValue {
    let same_site = if secure {
        cookie::SameSite::None
    } else {
        cookie::SameSite::Lax
    };
    let cookie = cookie::Cookie::build((SAML_TXN_COOKIE, value))
        .http_only(true)
        .same_site(same_site)
        .path("/api/v1/auth")
        .max_age(cookie::time::Duration::seconds(LOGIN_TXN_TTL_MS / 1000))
        .secure(secure)
        .build();
    HeaderValue::from_str(&cookie.to_string()).expect("saml txn cookie is always valid ASCII")
}

/// Minimal percent-encoding for a query component (provider names are
/// `[a-z0-9-]+`, so this is belt-and-braces).
fn urlencode(raw: &str) -> String {
    raw.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                vec![c]
            } else {
                format!("%{:02X}", c as u32).chars().collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_to_validation_rejects_offsite_targets() {
        assert_eq!(
            validate_return_to(Some("/runs/abc?tab=diff")),
            Some("/runs/abc?tab=diff".to_string())
        );
        assert_eq!(validate_return_to(Some("//evil.example")), None);
        assert_eq!(validate_return_to(Some("https://evil.example/")), None);
        assert_eq!(validate_return_to(Some("")), None);
        assert_eq!(validate_return_to(None), None);
        // Backslash bypasses: browsers normalize `\` to `/`.
        assert_eq!(validate_return_to(Some("/\\evil.example")), None);
        assert_eq!(validate_return_to(Some("\\\\evil.example")), None);
        assert_eq!(validate_return_to(Some("/foo\\bar")), None);
        // Control characters are rejected too.
        assert_eq!(validate_return_to(Some("/foo\nbar")), None);
    }
}
