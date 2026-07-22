//! OIDC relying party: authorization-code flow with PKCE and nonce.
//!
//! Discovery (which also pulls the JWKS) is fetched lazily and cached with a
//! TTL — an unreachable IdP must never crashloop server startup. ID-token
//! verification failures trigger one forced re-discovery and retry, which
//! covers IdP signing-key rotation without needing to parse `kid` misses out
//! of error variants.

use std::sync::Arc;
use std::time::{Duration, Instant};

use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
    CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreProviderMetadata, CoreRevocableToken, CoreRevocationErrorResponse,
    CoreTokenIntrospectionResponse, CoreTokenType,
};
use openidconnect::{
    AdditionalClaims, AuthorizationCode, Client, ClientId, ClientSecret, CsrfToken,
    EmptyExtraTokenFields, EndpointMaybeSet, EndpointNotSet, EndpointSet, IdTokenFields, IssuerUrl,
    Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope as OidcScope,
    StandardErrorResponse, StandardTokenResponse, TokenResponse,
};

use crate::sso::http::HttpClient;
use crate::sso::settings::OidcProviderSettings;
use crate::sso::{AssertedIdentity, SsoError};
use crate::storage::LoginTxn;

/// How long a cached discovery document (and its JWKS) stays fresh.
const DISCOVERY_TTL: Duration = Duration::from_secs(3600);

/// The full ID-token claim map, kept raw so the *configured* groups claim
/// can be read by name — serde cannot bind a field whose name is runtime
/// configuration.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct RawClaims(pub serde_json::Map<String, serde_json::Value>);

impl AdditionalClaims for RawClaims {}

type RawIdTokenFields = IdTokenFields<
    RawClaims,
    EmptyExtraTokenFields,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;
type RawTokenResponse = StandardTokenResponse<RawIdTokenFields, CoreTokenType>;
/// `CoreClient` with [`RawClaims`] in place of `EmptyAdditionalClaims`, in
/// the typestate `from_provider_metadata` leaves it in.
type RawClient = Client<
    RawClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    RawTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// Everything `begin` produced that the caller must persist in the login
/// transaction for the callback to verify.
pub struct OidcBegin {
    pub auth_url: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

struct CachedDiscovery {
    metadata: CoreProviderMetadata,
    fetched_at: Instant,
}

pub struct OidcProvider {
    pub cfg: OidcProviderSettings,
    clock_skew_secs: u64,
    http: HttpClient,
    discovery: tokio::sync::RwLock<Option<Arc<CachedDiscovery>>>,
}

impl std::fmt::Debug for OidcProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcProvider")
            .field("name", &self.cfg.name)
            .field("issuer", &self.cfg.issuer)
            .finish_non_exhaustive()
    }
}

impl OidcProvider {
    pub(crate) fn new(cfg: OidcProviderSettings, clock_skew_secs: u64, http: HttpClient) -> Self {
        OidcProvider {
            cfg,
            clock_skew_secs,
            http,
            discovery: tokio::sync::RwLock::new(None),
        }
    }

    /// The registered redirect URI for this provider.
    pub fn redirect_uri(&self, public_url: &str) -> String {
        format!(
            "{}/api/v1/auth/oidc/{}/callback",
            public_url.trim_end_matches('/'),
            self.cfg.name
        )
    }

    /// Cached discovery metadata (includes the JWKS), refreshed on TTL expiry
    /// or when `force_refresh` (signature-failure retry) demands it.
    async fn metadata(&self, force_refresh: bool) -> Result<Arc<CachedDiscovery>, SsoError> {
        if !force_refresh {
            let cached = self.discovery.read().await;
            if let Some(entry) = cached.as_ref() {
                if entry.fetched_at.elapsed() < DISCOVERY_TTL {
                    return Ok(entry.clone());
                }
            }
        }
        let issuer = IssuerUrl::new(self.cfg.issuer.clone())
            .map_err(|e| SsoError::Internal(anyhow::anyhow!("invalid issuer URL: {e}")))?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &self.http)
            .await
            .map_err(|e| SsoError::Provider(anyhow::anyhow!("OIDC discovery failed: {e}")))?;
        let entry = Arc::new(CachedDiscovery {
            metadata,
            fetched_at: Instant::now(),
        });
        *self.discovery.write().await = Some(entry.clone());
        Ok(entry)
    }

    fn client(
        &self,
        metadata: &CoreProviderMetadata,
        redirect_uri: &str,
    ) -> Result<RawClient, SsoError> {
        let redirect = RedirectUrl::new(redirect_uri.to_string())
            .map_err(|e| SsoError::Internal(anyhow::anyhow!("invalid redirect URI: {e}")))?;
        Ok(RawClient::from_provider_metadata(
            metadata.clone(),
            ClientId::new(self.cfg.client_id.clone()),
            Some(ClientSecret::new(self.cfg.client_secret.clone())),
        )
        .set_redirect_uri(redirect))
    }

    /// Build the IdP authorization URL for a new login transaction whose id
    /// doubles as the OAuth `state`.
    pub async fn begin(&self, redirect_uri: &str, txn_id: &str) -> Result<OidcBegin, SsoError> {
        let discovery = self.metadata(false).await?;
        let client = self.client(&discovery.metadata, redirect_uri)?;

        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let state = txn_id.to_string();
        let mut request = client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            move || CsrfToken::new(state),
            Nonce::new_random,
        );
        // authorize_url always includes `openid`; add the configured rest.
        for scope in &self.cfg.scopes {
            if scope != "openid" {
                request = request.add_scope(OidcScope::new(scope.clone()));
            }
        }
        let (auth_url, _state, nonce) = request.set_pkce_challenge(pkce_challenge).url();

        Ok(OidcBegin {
            auth_url: auth_url.to_string(),
            nonce: nonce.secret().clone(),
            pkce_verifier: pkce_verifier.secret().clone(),
        })
    }

    /// Exchange the authorization code and verify the ID token against the
    /// transaction's nonce/PKCE, returning the asserted identity.
    pub async fn complete(
        &self,
        code: &str,
        txn: &LoginTxn,
        redirect_uri: &str,
    ) -> Result<AssertedIdentity, SsoError> {
        let nonce = Nonce::new(
            txn.nonce
                .clone()
                .ok_or_else(|| SsoError::Internal(anyhow::anyhow!("login txn has no nonce")))?,
        );
        let pkce_verifier = txn
            .pkce_verifier
            .clone()
            .ok_or_else(|| SsoError::Internal(anyhow::anyhow!("login txn has no PKCE verifier")))?;

        let discovery = self.metadata(false).await?;
        let client = self.client(&discovery.metadata, redirect_uri)?;
        let token_response = client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .map_err(|e| SsoError::Provider(anyhow::anyhow!("no token endpoint: {e}")))?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(&self.http)
            .await
            .map_err(|e| SsoError::Provider(anyhow::anyhow!("token exchange failed: {e}")))?;

        let id_token = token_response
            .id_token()
            .ok_or_else(|| SsoError::Provider(anyhow::anyhow!("token response has no id_token")))?
            .clone();

        // Verify; on failure, force one re-discovery (JWKS rotation) + retry.
        let claims = match self.verify(&client, &id_token, &nonce) {
            Ok(claims) => claims,
            Err(first_err) => {
                let refreshed = self.metadata(true).await?;
                let client = self.client(&refreshed.metadata, redirect_uri)?;
                self.verify(&client, &id_token, &nonce)
                    .map_err(|_| first_err)?
            }
        };

        let email = claims.email().map(|e| e.to_string());
        // Only an explicitly-verified email may drive a security decision.
        // An absent `email_verified` claim is treated as unverified, not
        // trusted — otherwise an IdP that omits the claim (or lets a user
        // assert an arbitrary email) could satisfy a domain allowlist or
        // match an admin-email mapping. The raw email is still kept for
        // display / username derivation via `AssertedIdentity::email`.
        let email_verified = claims.email_verified() == Some(true);

        let display_name = claims
            .name()
            .and_then(|localized| localized.get(None))
            .map(|name| name.to_string());

        let groups = extract_groups(claims.additional_claims(), &self.cfg.groups_claim);

        Ok(AssertedIdentity {
            subject: claims.subject().to_string(),
            email,
            email_verified,
            display_name,
            groups,
        })
    }

    fn verify(
        &self,
        client: &RawClient,
        id_token: &openidconnect::IdToken<
            RawClaims,
            CoreGenderClaim,
            CoreJweContentEncryptionAlgorithm,
            CoreJwsSigningAlgorithm,
        >,
        nonce: &Nonce,
    ) -> Result<openidconnect::IdTokenClaims<RawClaims, CoreGenderClaim>, SsoError> {
        let skew = chrono::Duration::seconds(self.clock_skew_secs as i64);
        // Shifting the verifier's clock back by the skew tolerates an
        // IdP-issued `exp` that our (slightly fast) clock thinks just passed.
        let verifier = client
            .id_token_verifier()
            .set_time_fn(move || chrono::Utc::now() - skew);
        id_token
            .claims(&verifier, nonce)
            .cloned()
            .map_err(|e| SsoError::Provider(anyhow::anyhow!("id_token verification failed: {e}")))
    }
}

/// Read group names out of the raw claim map: an array of strings, or a
/// single string, under the configured claim name.
fn extract_groups(claims: &RawClaims, groups_claim: &str) -> Vec<String> {
    match claims.0.get(groups_claim) {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(serde_json::Value::String(single)) => vec![single.clone()],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_extraction_accepts_array_string_and_absence() {
        let mut map = serde_json::Map::new();
        map.insert("groups".into(), serde_json::json!(["a", "b", 3]));
        map.insert("role".into(), serde_json::json!("admins"));
        let claims = RawClaims(map);
        assert_eq!(extract_groups(&claims, "groups"), vec!["a", "b"]);
        assert_eq!(extract_groups(&claims, "role"), vec!["admins"]);
        assert!(extract_groups(&claims, "missing").is_empty());
    }
}
