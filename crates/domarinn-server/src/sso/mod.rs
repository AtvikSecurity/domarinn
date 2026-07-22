//! SSO: OIDC and SAML single sign-on.
//!
//! Structure:
//! * [`settings`] — env-var parsing (`DOMARINN_OIDC_*` / `DOMARINN_SAML_*`),
//! * [`mapping`] — claim/attribute → allow/role decisions,
//! * [`SsoRegistry`] — the configured providers, built once at startup and
//!   advertised to the UI through `GET /meta`.
//!
//! Provider protocol implementations land alongside this module (`oidc`,
//! `saml`); handlers live in `handlers` and are routed under
//! `/api/v1/auth/{oidc,saml}/{provider}/...`.

pub(crate) mod handlers;
pub mod http;
pub mod mapping;
pub mod oidc;
pub(crate) mod provision;
#[cfg(feature = "saml")]
pub mod saml;
pub mod settings;

pub use mapping::RoleMapping;
pub use settings::{
    parse_sso_settings, OidcProviderSettings, SamlIdpSource, SamlProviderSettings, SsoSettings,
};

use std::sync::Arc;

use crate::domain::SsoKind;
use crate::dto::meta::SsoProviderMeta;
use crate::sso::http::HttpClient;
use crate::sso::oidc::OidcProvider;

/// What an IdP asserted about the person who just authenticated, normalized
/// across protocols. `subject` is the stable IdP identifier (OIDC `sub`,
/// SAML `NameID`) that identities match on.
#[derive(Debug, Clone)]
pub struct AssertedIdentity {
    pub subject: String,
    pub email: Option<String>,
    /// Whether the IdP vouches for the email. Only a verified email may drive
    /// a security decision (domain allowlist, admin-by-email). OIDC sets this
    /// from `email_verified` (absent ⇒ false); a SAML email comes from the
    /// signed assertion, so it is trusted.
    pub email_verified: bool,
    pub display_name: Option<String>,
    pub groups: Vec<String>,
}

impl AssertedIdentity {
    /// The email only when the IdP vouches for it — what may be used for the
    /// domain allowlist and admin-by-email mapping.
    pub fn trusted_email(&self) -> Option<&str> {
        if self.email_verified {
            self.email.as_deref()
        } else {
            None
        }
    }
}

/// Why an SSO login failed. Redirect handlers surface only the
/// [`code`](SsoError::code) to the browser (`/login?sso_error=<code>`);
/// details go to the server log.
#[derive(Debug)]
pub enum SsoError {
    /// The IdP reported the user denied/failed authentication.
    AccessDenied,
    /// The callback's state/RelayState matched no in-flight transaction.
    InvalidState,
    /// The transaction (or assertion validity window) expired.
    Expired,
    /// The asserted email is outside the provider's allowed domains.
    EmailNotAllowed,
    /// The assertion was already consumed once.
    Replayed,
    /// The matched local account is disabled.
    UserDisabled,
    /// The IdP interaction itself failed (discovery, token exchange,
    /// signature or assertion validation).
    Provider(anyhow::Error),
    /// Our own storage/config failed.
    Internal(anyhow::Error),
}

impl SsoError {
    /// Stable machine code for the login page's error banner.
    pub fn code(&self) -> &'static str {
        match self {
            SsoError::AccessDenied => "access_denied",
            SsoError::InvalidState => "invalid_state",
            SsoError::Expired => "expired",
            SsoError::EmailNotAllowed => "email_not_allowed",
            SsoError::Replayed => "replayed",
            SsoError::UserDisabled => "account_disabled",
            SsoError::Provider(_) => "provider_error",
            SsoError::Internal(_) => "internal",
        }
    }
}

impl std::fmt::Display for SsoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SsoError::Provider(e) => write!(f, "{}: {e:#}", self.code()),
            SsoError::Internal(e) => write!(f, "{}: {e:#}", self.code()),
            other => f.write_str(other.code()),
        }
    }
}

impl From<anyhow::Error> for SsoError {
    fn from(e: anyhow::Error) -> Self {
        SsoError::Internal(e)
    }
}

/// The configured SSO providers. Built once in `AppState::new` — startup
/// fails fast on invalid configuration (including SAML IdP metadata that
/// cannot be fetched/parsed) so a typo can never silently drop a login
/// method. OIDC discovery itself stays lazy: IdP downtime must not
/// crashloop the server.
#[derive(Debug, Default)]
pub struct SsoRegistry {
    oidc: Vec<Arc<OidcProvider>>,
    #[cfg(feature = "saml")]
    saml: Vec<Arc<saml::SamlProvider>>,
    clock_skew_secs: u64,
}

impl SsoRegistry {
    /// Validate settings into a registry. Any configured provider requires
    /// `DOMARINN_PUBLIC_URL` — redirect URIs and the SAML ACS/entity URLs
    /// must be absolute and stable.
    pub async fn from_settings(
        sso: &SsoSettings,
        public_url: Option<&str>,
    ) -> anyhow::Result<SsoRegistry> {
        if sso.any() && public_url.is_none() {
            anyhow::bail!(
                "SSO providers are configured but DOMARINN_PUBLIC_URL is not set; \
                 it is required to build OIDC redirect URIs and SAML endpoint URLs"
            );
        }
        #[cfg(not(feature = "saml"))]
        if !sso.saml.is_empty() {
            anyhow::bail!(
                "DOMARINN_SAML_PROVIDERS is set but this binary was built without \
                 SAML support (cargo feature 'saml'); use the release/Docker build \
                 or rebuild with --features saml"
            );
        }

        let http = HttpClient::new()?;
        let oidc = sso
            .oidc
            .iter()
            .map(|cfg| {
                Arc::new(OidcProvider::new(
                    cfg.clone(),
                    sso.clock_skew_secs,
                    http.clone(),
                ))
            })
            .collect();

        #[cfg(feature = "saml")]
        let saml = {
            let mut providers = Vec::new();
            for cfg in &sso.saml {
                let public_url = public_url.expect("checked above: SSO requires a public URL");
                providers.push(Arc::new(
                    saml::SamlProvider::from_settings(cfg, public_url, sso.clock_skew_secs, &http)
                        .await?,
                ));
            }
            providers
        };

        Ok(SsoRegistry {
            oidc,
            #[cfg(feature = "saml")]
            saml,
            clock_skew_secs: sso.clock_skew_secs,
        })
    }

    /// Tolerance for token/assertion time validation.
    pub fn clock_skew_secs(&self) -> u64 {
        self.clock_skew_secs
    }

    /// The OIDC provider registered under `name`.
    pub fn oidc(&self, name: &str) -> Option<Arc<OidcProvider>> {
        self.oidc.iter().find(|p| p.cfg.name == name).cloned()
    }

    /// The SAML provider registered under `name`.
    #[cfg(feature = "saml")]
    pub fn saml(&self, name: &str) -> Option<Arc<saml::SamlProvider>> {
        self.saml.iter().find(|p| p.name == name).cloned()
    }

    /// Provider descriptors for `GET /meta` — exactly what the login page
    /// needs to render SSO buttons.
    pub fn descriptors(&self) -> Vec<SsoProviderMeta> {
        #[cfg_attr(not(feature = "saml"), allow(unused_mut))]
        let mut out: Vec<SsoProviderMeta> = self
            .oidc
            .iter()
            .map(|p| SsoProviderMeta {
                name: p.cfg.name.clone(),
                kind: SsoKind::Oidc,
                label: p.cfg.label.clone(),
                login_url: format!("/api/v1/auth/oidc/{}/start", p.cfg.name),
            })
            .collect();
        #[cfg(feature = "saml")]
        out.extend(self.saml.iter().map(|p| SsoProviderMeta {
            name: p.name.clone(),
            kind: SsoKind::Saml,
            label: p.label.clone(),
            login_url: format!("/api/v1/auth/saml/{}/start", p.name),
        }));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn oidc_settings() -> SsoSettings {
        let vars: HashMap<String, String> = [
            ("DOMARINN_OIDC_PROVIDERS", "google"),
            ("DOMARINN_OIDC_GOOGLE_ISSUER", "https://accounts.google.com"),
            ("DOMARINN_OIDC_GOOGLE_CLIENT_ID", "cid"),
            ("DOMARINN_OIDC_GOOGLE_CLIENT_SECRET", "sec"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        parse_sso_settings(&vars).unwrap()
    }

    #[tokio::test]
    async fn registry_requires_public_url_when_providers_exist() {
        let err = SsoRegistry::from_settings(&oidc_settings(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("DOMARINN_PUBLIC_URL"), "{err}");

        // No providers -> no public-url requirement.
        assert!(SsoRegistry::from_settings(&SsoSettings::default(), None)
            .await
            .is_ok());
    }

    #[cfg(not(feature = "saml"))]
    #[tokio::test]
    async fn saml_config_without_the_feature_fails_loudly() {
        let vars: HashMap<String, String> = [
            ("DOMARINN_SAML_PROVIDERS", "okta"),
            ("DOMARINN_SAML_OKTA_IDP_METADATA_URL", "https://m"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let settings = parse_sso_settings(&vars).unwrap();
        let err = SsoRegistry::from_settings(&settings, Some("https://r.example"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cargo feature 'saml'"), "{err}");
    }

    #[tokio::test]
    async fn descriptors_expose_login_urls() {
        let registry =
            SsoRegistry::from_settings(&oidc_settings(), Some("https://results.example.com"))
                .await
                .unwrap();
        let descriptors = registry.descriptors();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].name, "google");
        assert_eq!(descriptors[0].kind, SsoKind::Oidc);
        assert_eq!(descriptors[0].label, "Google");
        assert_eq!(descriptors[0].login_url, "/api/v1/auth/oidc/google/start");
    }

    #[test]
    fn sso_error_codes_are_stable() {
        assert_eq!(SsoError::AccessDenied.code(), "access_denied");
        assert_eq!(SsoError::InvalidState.code(), "invalid_state");
        assert_eq!(SsoError::Expired.code(), "expired");
        assert_eq!(SsoError::EmailNotAllowed.code(), "email_not_allowed");
        assert_eq!(SsoError::Replayed.code(), "replayed");
        assert_eq!(SsoError::UserDisabled.code(), "account_disabled");
        assert_eq!(
            SsoError::Provider(anyhow::anyhow!("x")).code(),
            "provider_error"
        );
        assert_eq!(SsoError::Internal(anyhow::anyhow!("x")).code(), "internal");
    }
}
