//! SAML 2.0 service provider, built on samael (xmlsec-backed signature
//! verification with XSW reduction).
//!
//! Compiled only with the `saml` cargo feature: samael's `xmlsec` feature
//! needs C libraries (libxmlsec1/libxml2/openssl) at build time, so plain
//! dev builds stay dependency-free while release/Docker builds ship SAML. A
//! binary built without the feature refuses SAML configuration at startup —
//! never silently.
//!
//! samael validates signatures (with an explicit algorithm allowlist),
//! issuer, destination, conditions/audience with clock skew, and
//! `InResponseTo`; this module adds what it deliberately leaves to the
//! caller: request-id tracking (the login-transaction table), the
//! assertion-replay cache, and attribute → identity mapping.

use anyhow::Context;
use chrono::Duration;
use samael::metadata::{EntityDescriptor, HTTP_REDIRECT_BINDING};
use samael::schema::Assertion;
use samael::service_provider::ServiceProvider;

use crate::sso::http::HttpClient;
use crate::sso::settings::{SamlIdpSource, SamlProviderSettings};
use crate::sso::{AssertedIdentity, SsoError};

const NAME_ID_FORMAT_EMAIL: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";
/// Replay-cache retention when the assertion carries no `NotOnOrAfter`.
const REPLAY_FALLBACK_MS: i64 = 60 * 60 * 1000;

/// What `begin` produced: where to send the browser, and the AuthnRequest id
/// the callback must see as `InResponseTo`.
pub struct SamlBegin {
    pub redirect_url: String,
    pub request_id: String,
}

/// A verified login: the identity plus the replay-cache bookkeeping.
pub struct SamlLogin {
    pub asserted: AssertedIdentity,
    pub assertion_id: String,
    /// Epoch ms after which the assertion id can leave the replay cache.
    pub replay_expiry_ms: i64,
}

pub struct SamlProvider {
    pub name: String,
    pub label: String,
    pub mapping: crate::sso::RoleMapping,
    pub allow_idp_initiated: bool,
    email_attr: Option<String>,
    groups_attr: String,
    idp_sso_url: String,
    sp: ServiceProvider,
}

impl std::fmt::Debug for SamlProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SamlProvider")
            .field("name", &self.name)
            .field("idp_sso_url", &self.idp_sso_url)
            .finish_non_exhaustive()
    }
}

impl SamlProvider {
    /// Resolve IdP metadata (URL fetch / file / inline cert) and build the
    /// service provider. Fails fast on anything that would otherwise degrade
    /// silently — most importantly an IdP without signing certificates,
    /// which samael would treat as "skip signature verification".
    pub async fn from_settings(
        cfg: &SamlProviderSettings,
        public_url: &str,
        clock_skew_secs: u64,
        http: &HttpClient,
    ) -> anyhow::Result<SamlProvider> {
        let metadata_xml = match &cfg.idp {
            SamlIdpSource::MetadataUrl(url) => http.get_text(url).await.with_context(|| {
                format!("fetching SAML IdP metadata for '{}' from {url}", cfg.name)
            })?,
            SamlIdpSource::MetadataFile(path) => {
                std::fs::read_to_string(path).with_context(|| {
                    format!("reading SAML IdP metadata for '{}' from {path}", cfg.name)
                })?
            }
            SamlIdpSource::Inline { sso_url, cert_pem } => {
                inline_idp_metadata(&cfg.name, sso_url, cert_pem)?
            }
        };
        let idp_metadata: EntityDescriptor = metadata_xml
            .parse()
            .map_err(|e| anyhow::anyhow!("parsing SAML IdP metadata for '{}': {e}", cfg.name))?;

        let base = public_url.trim_end_matches('/');
        let entity_id = cfg
            .sp_entity_id
            .clone()
            .unwrap_or_else(|| format!("{base}/api/v1/auth/saml/{}/metadata", cfg.name));
        let acs_url = format!("{base}/api/v1/auth/saml/{}/acs", cfg.name);
        let metadata_url = format!("{base}/api/v1/auth/saml/{}/metadata", cfg.name);

        let skew = Duration::seconds(clock_skew_secs as i64);
        let sp = ServiceProvider {
            entity_id: Some(entity_id),
            key: None,
            certificate: None,
            intermediates: None,
            metadata_url: Some(metadata_url),
            acs_url: Some(acs_url),
            slo_url: None,
            idp_metadata,
            authn_name_id_format: Some(NAME_ID_FORMAT_EMAIL.to_string()),
            metadata_valid_duration: None,
            force_authn: false,
            allow_idp_initiated: cfg.allow_idp_initiated,
            contact_person: None,
            // The response should arrive seconds after issuance; allow the
            // configured skew on top of samael's 90s default.
            max_issue_delay: Duration::seconds(90) + skew,
            max_clock_skew: skew,
            allowed_signature_algorithms: Some(vec![
                samael::crypto::AllowedSignatureAlgorithm::RsaSha256,
                samael::crypto::AllowedSignatureAlgorithm::RsaSha384,
                samael::crypto::AllowedSignatureAlgorithm::RsaSha512,
                samael::crypto::AllowedSignatureAlgorithm::EcdsaSha256,
            ]),
        };

        let idp_sso_url = sp
            .sso_binding_location(HTTP_REDIRECT_BINDING)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "SAML IdP metadata for '{}' has no HTTP-Redirect SingleSignOnService",
                    cfg.name
                )
            })?;
        // Fail closed: without signing certs samael skips verification.
        let certs = sp
            .idp_signing_certs()
            .map_err(|e| anyhow::anyhow!("reading IdP signing certs for '{}': {e}", cfg.name))?;
        if certs.is_none_or(|c| c.is_empty()) {
            anyhow::bail!(
                "SAML IdP metadata for '{}' contains no signing certificate; \
                 refusing a configuration that cannot verify response signatures",
                cfg.name
            );
        }

        Ok(SamlProvider {
            name: cfg.name.clone(),
            label: cfg.label.clone(),
            mapping: cfg.mapping.clone(),
            allow_idp_initiated: cfg.allow_idp_initiated,
            email_attr: cfg.email_attr.clone(),
            groups_attr: cfg.groups_attr.clone(),
            idp_sso_url,
            sp,
        })
    }

    /// Build the redirect-binding AuthnRequest URL. `relay_state` is the
    /// login-transaction id.
    pub fn begin(&self, relay_state: &str) -> Result<SamlBegin, SsoError> {
        let request = self
            .sp
            .make_authentication_request(&self.idp_sso_url)
            .map_err(|e| SsoError::Internal(anyhow::anyhow!("building AuthnRequest: {e}")))?;
        let request_id = request.id.clone();
        let redirect_url = request
            .redirect(relay_state)
            .map_err(|e| SsoError::Internal(anyhow::anyhow!("encoding AuthnRequest: {e}")))?
            .ok_or_else(|| SsoError::Internal(anyhow::anyhow!("AuthnRequest produced no URL")))?
            .to_string();
        Ok(SamlBegin {
            redirect_url,
            request_id,
        })
    }

    /// Verify a POSTed `SAMLResponse` (signature, issuer, destination,
    /// conditions, audience, `InResponseTo`) and map it to an identity. The
    /// caller still owns the replay-cache insert.
    pub fn complete(
        &self,
        saml_response_b64: &str,
        expected_request_id: Option<&str>,
    ) -> Result<SamlLogin, SsoError> {
        // Surface an actionable error for encrypted assertions instead of a
        // generic parse failure: every supported IdP can turn encryption off.
        if let Ok(bytes) = base64_decode(saml_response_b64) {
            if String::from_utf8_lossy(&bytes).contains("EncryptedAssertion") {
                return Err(SsoError::Provider(anyhow::anyhow!(
                    "the IdP sent an encrypted assertion, which is not supported; \
                     disable assertion encryption for this application at the IdP"
                )));
            }
        }

        let ids: Vec<&str> = expected_request_id.into_iter().collect();
        let possible_ids = if ids.is_empty() {
            None
        } else {
            Some(ids.as_slice())
        };
        let assertion = self
            .sp
            .parse_base64_response(saml_response_b64.trim(), possible_ids)
            .map_err(|e| SsoError::Provider(anyhow::anyhow!("SAML response rejected: {e}")))?;

        let login = self.map_assertion(&assertion)?;
        Ok(login)
    }

    /// The SP metadata document IdPs import. Emitted directly rather than
    /// via samael's builder, which requires a SingleLogoutService endpoint —
    /// we do not implement SLO and must not advertise one.
    pub fn sp_metadata_xml(&self) -> Result<String, SsoError> {
        let entity_id = self.sp.entity_id.as_deref().unwrap_or_default();
        let acs_url = self.sp.acs_url.as_deref().unwrap_or_default();
        Ok(format!(
            r#"<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{}">
  <md:SPSSODescriptor AuthnRequestsSigned="false" WantAssertionsSigned="true" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:NameIDFormat>{NAME_ID_FORMAT_EMAIL}</md:NameIDFormat>
    <md:AssertionConsumerService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="{}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>"#,
            xml_escape(entity_id),
            xml_escape(acs_url),
        ))
    }

    fn map_assertion(&self, assertion: &Assertion) -> Result<SamlLogin, SsoError> {
        let name_id = assertion
            .subject
            .as_ref()
            .and_then(|s| s.name_id.as_ref())
            .ok_or_else(|| {
                SsoError::Provider(anyhow::anyhow!("assertion has no Subject NameID"))
            })?;
        let subject = name_id.value.clone();

        // Email: explicit attribute override, else an emailAddress-format
        // NameID, else the conventional `email`/`mail` attributes.
        let email = match &self.email_attr {
            Some(attr) => first_attribute(assertion, attr),
            None => {
                if name_id.format.as_deref() == Some(NAME_ID_FORMAT_EMAIL) {
                    Some(subject.clone())
                } else {
                    first_attribute(assertion, "email")
                        .or_else(|| first_attribute(assertion, "mail"))
                }
            }
        };

        let display_name = first_attribute(assertion, "displayName")
            .or_else(|| first_attribute(assertion, "cn"))
            .or_else(|| first_attribute(assertion, "name"));

        let groups = attribute_values(assertion, &self.groups_attr);

        let replay_expiry_ms = assertion
            .conditions
            .as_ref()
            .and_then(|c| c.not_on_or_after)
            .map(|t| {
                t.timestamp_millis()
                    + Duration::seconds(self.sp.max_clock_skew.num_seconds()).num_milliseconds()
            })
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() + REPLAY_FALLBACK_MS);

        Ok(SamlLogin {
            asserted: AssertedIdentity {
                subject,
                email,
                // The email comes from the signature-verified assertion, so
                // the IdP vouches for it.
                email_verified: true,
                display_name,
                groups,
            },
            assertion_id: assertion.id.clone(),
            replay_expiry_ms,
        })
    }
}

/// All values of every attribute named `name`, across statements (covers
/// both multi-valued attributes and repeated single-value ones).
fn attribute_values(assertion: &Assertion, name: &str) -> Vec<String> {
    assertion
        .attribute_statements
        .iter()
        .flatten()
        .flat_map(|statement| &statement.attributes)
        .filter(|attribute| {
            attribute.name.as_deref() == Some(name)
                || attribute.friendly_name.as_deref() == Some(name)
        })
        .flat_map(|attribute| &attribute.values)
        .filter_map(|value| value.value.clone())
        .collect()
}

fn first_attribute(assertion: &Assertion, name: &str) -> Option<String> {
    attribute_values(assertion, name).into_iter().next()
}

/// Wrap an explicit SSO URL + signing cert into minimal IdP metadata so the
/// inline config path reuses the exact same parsing/validation as real
/// metadata. The PEM body *is* the base64 DER the metadata format wants.
fn inline_idp_metadata(name: &str, sso_url: &str, cert_pem: &str) -> anyhow::Result<String> {
    let cert_b64: String = cert_pem
        .lines()
        .filter(|line| !line.contains("-----"))
        .collect::<Vec<_>>()
        .join("");
    if cert_b64.is_empty() {
        anyhow::bail!("SAML provider '{name}': IDP_CERT contains no certificate body");
    }
    Ok(format!(
        r#"<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{sso_url}">
  <md:IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="signing">
      <ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
        <ds:X509Data><ds:X509Certificate>{cert_b64}</ds:X509Certificate></ds:X509Data>
      </ds:KeyInfo>
    </md:KeyDescriptor>
    <md:SingleSignOnService Binding="{HTTP_REDIRECT_BINDING}" Location="{sso_url}"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#
    ))
}

fn base64_decode(raw: &str) -> Result<Vec<u8>, anyhow::Error> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.decode(raw.trim())?)
}

/// Minimal XML attribute/text escaping for the operator-supplied URLs
/// embedded in the SP metadata template.
fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
