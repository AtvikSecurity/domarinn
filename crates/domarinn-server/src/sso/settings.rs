//! Env-var parsing for SSO providers.
//!
//! The whole schema (all `DOMARINN_OIDC_*` / `DOMARINN_SAML_*` vars) is
//! parsed from a plain `HashMap` so every rule is unit-testable without
//! touching the process environment, mirroring `parse_auth_mode_env`.
//! Parsing is fail-loud: a listed provider with a missing required var, an
//! invalid name, or an unparseable value aborts startup naming the exact
//! variable — a typo must never silently drop a login method.

use std::collections::HashMap;

use anyhow::{bail, Context};

use crate::sso::mapping::RoleMapping;

/// Default `DOMARINN_SSO_CLOCK_SKEW_SECS`.
const DEFAULT_CLOCK_SKEW_SECS: u64 = 60;
/// Default OIDC scopes.
const DEFAULT_OIDC_SCOPES: &[&str] = &["openid", "email", "profile"];
/// Default OIDC groups claim / SAML groups attribute name.
const DEFAULT_GROUPS_KEY: &str = "groups";

/// Everything `DOMARINN_OIDC_*` / `DOMARINN_SAML_*` configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoSettings {
    pub oidc: Vec<OidcProviderSettings>,
    pub saml: Vec<SamlProviderSettings>,
    /// Tolerance applied to token/assertion time validation, seconds.
    pub clock_skew_secs: u64,
}

impl Default for SsoSettings {
    fn default() -> Self {
        SsoSettings {
            oidc: Vec::new(),
            saml: Vec::new(),
            clock_skew_secs: DEFAULT_CLOCK_SKEW_SECS,
        }
    }
}

impl SsoSettings {
    /// Whether any SSO provider is configured at all.
    pub fn any(&self) -> bool {
        !self.oidc.is_empty() || !self.saml.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcProviderSettings {
    /// URL-safe provider name from `DOMARINN_OIDC_PROVIDERS` (`[a-z0-9-]+`).
    pub name: String,
    /// Human label for the login button.
    pub label: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
    /// The ID-token claim holding group names.
    pub groups_claim: String,
    pub mapping: RoleMapping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamlProviderSettings {
    pub name: String,
    pub label: String,
    pub idp: SamlIdpSource,
    /// SP entity id; defaults to the SP metadata URL when unset.
    pub sp_entity_id: Option<String>,
    /// Assertion attribute holding the email. `None` = use the NameID when
    /// its format is emailAddress, else the `email`/`mail` attribute.
    pub email_attr: Option<String>,
    /// Assertion attribute holding group names.
    pub groups_attr: String,
    /// Accept responses without an `InResponseTo` (IdP-initiated SSO).
    pub allow_idp_initiated: bool,
    pub mapping: RoleMapping,
}

/// Where the IdP's SSO URL + signing certificates come from. Exactly one
/// source must be configured per provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SamlIdpSource {
    /// Fetch the IdP metadata XML from a URL at startup.
    MetadataUrl(String),
    /// Read the IdP metadata XML from a local file at startup.
    MetadataFile(String),
    /// Explicit SSO redirect URL + signing certificate (PEM).
    Inline { sso_url: String, cert_pem: String },
}

/// Parse the full SSO env schema out of `vars`.
pub fn parse_sso_settings(vars: &HashMap<String, String>) -> anyhow::Result<SsoSettings> {
    let get = |key: String| vars.get(&key).map(|v| v.trim()).filter(|v| !v.is_empty());

    let clock_skew_secs = match get("DOMARINN_SSO_CLOCK_SKEW_SECS".to_string()) {
        None => DEFAULT_CLOCK_SKEW_SECS,
        Some(raw) => raw
            .parse::<u64>()
            .with_context(|| format!("invalid DOMARINN_SSO_CLOCK_SKEW_SECS '{raw}'"))?,
    };

    let mut oidc = Vec::new();
    for name in provider_names(vars, "DOMARINN_OIDC_PROVIDERS")? {
        let stem = env_stem(&name);
        let var = |suffix: &str| format!("DOMARINN_OIDC_{stem}_{suffix}");
        let required = |suffix: &str| -> anyhow::Result<String> {
            get(var(suffix))
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("OIDC provider '{name}' is missing {}", var(suffix)))
        };
        oidc.push(OidcProviderSettings {
            label: get(var("LABEL"))
                .map(str::to_string)
                .unwrap_or_else(|| default_label(&name)),
            issuer: required("ISSUER")?,
            client_id: required("CLIENT_ID")?,
            client_secret: required("CLIENT_SECRET")?,
            scopes: get(var("SCOPES"))
                .map(|raw| raw.split_whitespace().map(str::to_string).collect())
                .unwrap_or_else(|| DEFAULT_OIDC_SCOPES.iter().map(|s| s.to_string()).collect()),
            groups_claim: get(var("GROUPS_CLAIM"))
                .map(str::to_string)
                .unwrap_or_else(|| DEFAULT_GROUPS_KEY.to_string()),
            mapping: parse_mapping(&get, &var),
            name,
        });
    }

    let mut saml = Vec::new();
    for name in provider_names(vars, "DOMARINN_SAML_PROVIDERS")? {
        let stem = env_stem(&name);
        let var = |suffix: &str| format!("DOMARINN_SAML_{stem}_{suffix}");

        let metadata_url = get(var("IDP_METADATA_URL")).map(str::to_string);
        let metadata_file = get(var("IDP_METADATA_FILE")).map(str::to_string);
        let sso_url = get(var("IDP_SSO_URL")).map(str::to_string);
        let cert = get(var("IDP_CERT")).map(str::to_string);
        let idp = match (metadata_url, metadata_file, sso_url, cert) {
            (Some(url), None, None, None) => SamlIdpSource::MetadataUrl(url),
            (None, Some(file), None, None) => SamlIdpSource::MetadataFile(file),
            (None, None, Some(sso_url), Some(cert_pem)) => {
                SamlIdpSource::Inline { sso_url, cert_pem }
            }
            (None, None, None, None) => bail!(
                "SAML provider '{name}' needs exactly one IdP source: {}, {}, or {} + {}",
                var("IDP_METADATA_URL"),
                var("IDP_METADATA_FILE"),
                var("IDP_SSO_URL"),
                var("IDP_CERT"),
            ),
            (None, None, Some(_), None) | (None, None, None, Some(_)) => bail!(
                "SAML provider '{name}': {} and {} must be set together",
                var("IDP_SSO_URL"),
                var("IDP_CERT"),
            ),
            _ => bail!(
                "SAML provider '{name}' has multiple IdP sources configured; \
                 set exactly one of {}, {}, or {} + {}",
                var("IDP_METADATA_URL"),
                var("IDP_METADATA_FILE"),
                var("IDP_SSO_URL"),
                var("IDP_CERT"),
            ),
        };

        let allow_idp_initiated = match get(var("ALLOW_IDP_INITIATED")) {
            None => false,
            Some("true") | Some("1") | Some("yes") => true,
            Some("false") | Some("0") | Some("no") => false,
            Some(other) => bail!(
                "invalid {} '{other}'; expected one of: true, false, 1, 0, yes, no",
                var("ALLOW_IDP_INITIATED")
            ),
        };

        saml.push(SamlProviderSettings {
            label: get(var("LABEL"))
                .map(str::to_string)
                .unwrap_or_else(|| default_label(&name)),
            idp,
            sp_entity_id: get(var("SP_ENTITY_ID")).map(str::to_string),
            email_attr: get(var("EMAIL_ATTR")).map(str::to_string),
            groups_attr: get(var("GROUPS_ATTR"))
                .map(str::to_string)
                .unwrap_or_else(|| DEFAULT_GROUPS_KEY.to_string()),
            allow_idp_initiated,
            mapping: parse_mapping(&get, &var),
            name,
        });
    }

    Ok(SsoSettings {
        oidc,
        saml,
        clock_skew_secs,
    })
}

/// The provider-name list from e.g. `DOMARINN_OIDC_PROVIDERS=google,authentik`.
/// Names must be `[a-z0-9-]+` (which also guarantees distinct names can never
/// collide after the `-` → `_` env-stem mapping) and unique within the list.
fn provider_names(vars: &HashMap<String, String>, list_var: &str) -> anyhow::Result<Vec<String>> {
    let Some(raw) = vars
        .get(list_var)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for name in raw.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            bail!(
                "invalid provider name '{name}' in {list_var}; \
                 names must match [a-z0-9-]+ (lowercase, digits, hyphens)"
            );
        }
        if names.iter().any(|existing| existing == name) {
            bail!("duplicate provider name '{name}' in {list_var}");
        }
        names.push(name.to_string());
    }
    Ok(names)
}

/// `my-idp` → `MY_IDP`, the stem used in per-provider variable names.
fn env_stem(name: &str) -> String {
    name.to_uppercase().replace('-', "_")
}

/// Default button label: the name with its first letter capitalized.
fn default_label(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn parse_mapping<'a>(
    get: &impl Fn(String) -> Option<&'a str>,
    var: &impl Fn(&str) -> String,
) -> RoleMapping {
    let list = |suffix: &str| -> Vec<String> {
        get(var(suffix))
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    RoleMapping {
        admin_groups: list("ADMIN_GROUPS"),
        admin_emails: list("ADMIN_EMAILS"),
        allowed_email_domains: list("ALLOWED_EMAIL_DOMAINS"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn no_sso_vars_parse_to_empty_settings() {
        let settings = parse_sso_settings(&HashMap::new()).unwrap();
        assert!(!settings.any());
        assert_eq!(settings.clock_skew_secs, 60);
    }

    #[test]
    fn full_oidc_provider_parses_with_defaults_and_overrides() {
        let settings = parse_sso_settings(&vars(&[
            ("DOMARINN_OIDC_PROVIDERS", "google,my-idp"),
            ("DOMARINN_OIDC_GOOGLE_ISSUER", "https://accounts.google.com"),
            ("DOMARINN_OIDC_GOOGLE_CLIENT_ID", "cid"),
            ("DOMARINN_OIDC_GOOGLE_CLIENT_SECRET", "sec"),
            (
                "DOMARINN_OIDC_GOOGLE_ADMIN_EMAILS",
                "ops@example.com, x@y.z",
            ),
            ("DOMARINN_OIDC_MY_IDP_ISSUER", "https://idp.example"),
            ("DOMARINN_OIDC_MY_IDP_CLIENT_ID", "cid2"),
            ("DOMARINN_OIDC_MY_IDP_CLIENT_SECRET", "sec2"),
            ("DOMARINN_OIDC_MY_IDP_LABEL", "Corp IdP"),
            ("DOMARINN_OIDC_MY_IDP_SCOPES", "openid email groups"),
            ("DOMARINN_OIDC_MY_IDP_GROUPS_CLAIM", "roles"),
            ("DOMARINN_OIDC_MY_IDP_ADMIN_GROUPS", "admins"),
            ("DOMARINN_OIDC_MY_IDP_ALLOWED_EMAIL_DOMAINS", "example.com"),
        ]))
        .unwrap();

        assert_eq!(settings.oidc.len(), 2);
        let google = &settings.oidc[0];
        assert_eq!(google.name, "google");
        assert_eq!(google.label, "Google");
        assert_eq!(google.scopes, vec!["openid", "email", "profile"]);
        assert_eq!(google.groups_claim, "groups");
        assert_eq!(
            google.mapping.admin_emails,
            vec!["ops@example.com", "x@y.z"]
        );

        let corp = &settings.oidc[1];
        assert_eq!(corp.name, "my-idp");
        assert_eq!(corp.label, "Corp IdP");
        assert_eq!(corp.scopes, vec!["openid", "email", "groups"]);
        assert_eq!(corp.groups_claim, "roles");
        assert_eq!(corp.mapping.admin_groups, vec!["admins"]);
        assert_eq!(corp.mapping.allowed_email_domains, vec!["example.com"]);
    }

    #[test]
    fn missing_required_oidc_var_names_it_exactly() {
        let err = parse_sso_settings(&vars(&[
            ("DOMARINN_OIDC_PROVIDERS", "google"),
            ("DOMARINN_OIDC_GOOGLE_ISSUER", "https://accounts.google.com"),
            ("DOMARINN_OIDC_GOOGLE_CLIENT_ID", "cid"),
        ]))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("DOMARINN_OIDC_GOOGLE_CLIENT_SECRET"),
            "{err}"
        );
    }

    #[test]
    fn invalid_and_duplicate_provider_names_are_rejected() {
        let err = parse_sso_settings(&vars(&[("DOMARINN_OIDC_PROVIDERS", "My_Idp")])).unwrap_err();
        assert!(err.to_string().contains("My_Idp"), "{err}");

        let err =
            parse_sso_settings(&vars(&[("DOMARINN_OIDC_PROVIDERS", "google,google")])).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn saml_requires_exactly_one_idp_source() {
        // None configured.
        let err = parse_sso_settings(&vars(&[("DOMARINN_SAML_PROVIDERS", "okta")])).unwrap_err();
        assert!(err.to_string().contains("exactly one IdP source"), "{err}");

        // Two configured.
        let err = parse_sso_settings(&vars(&[
            ("DOMARINN_SAML_PROVIDERS", "okta"),
            ("DOMARINN_SAML_OKTA_IDP_METADATA_URL", "https://m"),
            ("DOMARINN_SAML_OKTA_IDP_METADATA_FILE", "/etc/okta.xml"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("multiple IdP sources"), "{err}");

        // SSO URL without cert.
        let err = parse_sso_settings(&vars(&[
            ("DOMARINN_SAML_PROVIDERS", "okta"),
            ("DOMARINN_SAML_OKTA_IDP_SSO_URL", "https://sso"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");
    }

    #[test]
    fn saml_provider_parses_with_defaults() {
        let settings = parse_sso_settings(&vars(&[
            ("DOMARINN_SAML_PROVIDERS", "okta"),
            ("DOMARINN_SAML_OKTA_IDP_METADATA_URL", "https://m"),
            ("DOMARINN_SAML_OKTA_ADMIN_GROUPS", "sec-ops"),
        ]))
        .unwrap();
        let okta = &settings.saml[0];
        assert_eq!(okta.label, "Okta");
        assert_eq!(okta.idp, SamlIdpSource::MetadataUrl("https://m".into()));
        assert_eq!(okta.groups_attr, "groups");
        assert!(!okta.allow_idp_initiated);
        assert_eq!(okta.mapping.admin_groups, vec!["sec-ops"]);
    }

    #[test]
    fn bad_bool_and_bad_skew_fail_loud() {
        let err = parse_sso_settings(&vars(&[
            ("DOMARINN_SAML_PROVIDERS", "okta"),
            ("DOMARINN_SAML_OKTA_IDP_METADATA_URL", "https://m"),
            ("DOMARINN_SAML_OKTA_ALLOW_IDP_INITIATED", "ture"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("ALLOW_IDP_INITIATED"), "{err}");

        let err =
            parse_sso_settings(&vars(&[("DOMARINN_SSO_CLOCK_SKEW_SECS", "soon")])).unwrap_err();
        assert!(err.to_string().contains("CLOCK_SKEW"), "{err}");
    }
}
