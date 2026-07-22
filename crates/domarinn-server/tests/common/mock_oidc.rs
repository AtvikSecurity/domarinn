//! In-process OIDC identity provider for integration tests.
//!
//! Serves discovery, JWKS, and token endpoints on `127.0.0.1:0` and mints
//! RS256 id_tokens. The signing key (`tests/fixtures/oidc/
//! test-idp-signing-key.pem`) is TEST-ONLY committed material — it signs
//! nothing outside this mock and must never be reused elsewhere.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreClaimName, CoreGenderClaim, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreResponseType, CoreRsaPrivateSigningKey,
    CoreSubjectIdentifierType, CoreTokenType,
};
use openidconnect::{
    AccessToken, AdditionalClaims, Audience, AuthUrl, EmptyAdditionalProviderMetadata,
    EmptyExtraTokenFields, EndUserEmail, EndUserName, IdToken, IdTokenClaims, IdTokenFields,
    IssuerUrl, JsonWebKeyId, JsonWebKeySetUrl, Nonce, PrivateSigningKey, ResponseTypes,
    StandardClaims, StandardTokenResponse, SubjectIdentifier, TokenUrl,
};
use serde::{Deserialize, Serialize};

/// Additional claims the mock puts in its id_tokens; wire-compatible with
/// the server's raw claim map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestClaims {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<String>>,
}

impl AdditionalClaims for TestClaims {}

type TestIdTokenFields = IdTokenFields<
    TestClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;
type TestTokenResponse = StandardTokenResponse<TestIdTokenFields, CoreTokenType>;

/// What the next token exchange for a given `code` should assert.
#[derive(Debug, Clone)]
pub struct TokenSpec {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub groups: Option<Vec<String>>,
    /// Must match the nonce from the authorize URL of the login being tested.
    pub nonce: String,
    /// The audience (the RP's client id).
    pub client_id: String,
}

#[derive(Default)]
struct IdpState {
    tokens: HashMap<String, TokenSpec>,
}

#[derive(Clone)]
struct IdpShared {
    issuer: String,
    key_pem: &'static str,
    state: Arc<Mutex<IdpState>>,
}

pub struct MockIdp {
    pub issuer: String,
    state: Arc<Mutex<IdpState>>,
}

const KEY_PEM: &str = include_str!("../fixtures/oidc/test-idp-signing-key.pem");
const KEY_ID: &str = "test-key";

impl MockIdp {
    /// Bind on an ephemeral port and serve until the test process exits.
    pub async fn spawn() -> MockIdp {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock idp");
        let addr = listener.local_addr().expect("local addr");
        let issuer = format!("http://{addr}");
        let state = Arc::new(Mutex::new(IdpState::default()));

        let shared = IdpShared {
            issuer: issuer.clone(),
            key_pem: KEY_PEM,
            state: state.clone(),
        };
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(shared);
        tokio::spawn(async move {
            axum::serve(listener, router).await.expect("mock idp serve");
        });

        MockIdp { issuer, state }
    }

    /// Arm the token endpoint: exchanging `code` mints an id_token per `spec`.
    pub fn expect_token(&self, code: &str, spec: TokenSpec) {
        self.state
            .lock()
            .unwrap()
            .tokens
            .insert(code.to_string(), spec);
    }
}

fn signing_key() -> CoreRsaPrivateSigningKey {
    CoreRsaPrivateSigningKey::from_pem(KEY_PEM, Some(JsonWebKeyId::new(KEY_ID.to_string())))
        .expect("test signing key parses")
}

async fn discovery(State(shared): State<IdpShared>) -> impl IntoResponse {
    let metadata = CoreProviderMetadata::new(
        IssuerUrl::new(shared.issuer.clone()).unwrap(),
        AuthUrl::new(format!("{}/authorize", shared.issuer)).unwrap(),
        JsonWebKeySetUrl::new(format!("{}/jwks", shared.issuer)).unwrap(),
        vec![ResponseTypes::new(vec![CoreResponseType::Code])],
        vec![CoreSubjectIdentifierType::Public],
        vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
        EmptyAdditionalProviderMetadata {},
    )
    .set_token_endpoint(Some(
        TokenUrl::new(format!("{}/token", shared.issuer)).unwrap(),
    ))
    .set_claims_supported(Some(vec![
        CoreClaimName::new("sub".to_string()),
        CoreClaimName::new("email".to_string()),
        CoreClaimName::new("groups".to_string()),
    ]));
    Json(metadata)
}

async fn jwks(State(_shared): State<IdpShared>) -> impl IntoResponse {
    let jwks = CoreJsonWebKeySet::new(vec![signing_key().as_verification_key()]);
    Json(jwks)
}

#[derive(Debug, Deserialize)]
struct TokenForm {
    grant_type: String,
    code: String,
}

async fn token(
    State(shared): State<IdpShared>,
    axum::Form(form): axum::Form<TokenForm>,
) -> axum::response::Response {
    assert_eq!(form.grant_type, "authorization_code");
    let Some(spec) = shared.state.lock().unwrap().tokens.remove(&form.code) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid_grant" })),
        )
            .into_response();
    };

    let mut standard = StandardClaims::new(SubjectIdentifier::new(spec.sub));
    if let Some(email) = spec.email {
        standard = standard.set_email(Some(EndUserEmail::new(email)));
    }
    standard = standard.set_email_verified(spec.email_verified);
    if let Some(name) = spec.name {
        standard = standard.set_name(Some(EndUserName::new(name).into()));
    }

    let claims = IdTokenClaims::new(
        IssuerUrl::new(shared.issuer.clone()).unwrap(),
        vec![Audience::new(spec.client_id)],
        Utc::now() + Duration::minutes(5),
        Utc::now(),
        standard,
        TestClaims {
            groups: spec.groups,
        },
    )
    .set_nonce(Some(Nonce::new(spec.nonce)));

    let id_token = IdToken::<
        TestClaims,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >::new(
        claims,
        &signing_key(),
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        None,
        None,
    )
    .expect("sign id_token");

    let response = TestTokenResponse::new(
        AccessToken::new("mock-access-token".to_string()),
        CoreTokenType::Bearer,
        TestIdTokenFields::new(Some(id_token), EmptyExtraTokenFields {}),
    );
    Json(response).into_response()
}
