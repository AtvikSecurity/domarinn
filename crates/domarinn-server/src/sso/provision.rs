//! JIT provisioning and role re-sync, shared by the OIDC and SAML flows.

use crate::domain::SsoKind;
use crate::sso::{AssertedIdentity, RoleMapping, SsoError};
use crate::storage::{NewIdentity, Storage, UpdateUserOutcome, UserRow};

/// Resolve (or create) the local user behind a verified IdP assertion.
///
/// Matching is strictly `(provider_key, subject)` — never by email, which an
/// attacker who controls a mailbox at one IdP could otherwise use to take
/// over an account provisioned by another. On every login the identity
/// snapshot is refreshed and the role is re-synced from the mapping, so the
/// IdP stays the source of truth for SSO users — with one exception: the
/// last enabled admin is never auto-demoted (the same invariant the admin
/// API enforces), it is logged instead.
pub(crate) async fn provision(
    storage: &Storage,
    provider_key: &str,
    kind: SsoKind,
    asserted: &AssertedIdentity,
    mapping: &RoleMapping,
) -> Result<UserRow, SsoError> {
    // Only a verified email may gate access or grant admin; the raw email is
    // still stored / used for username derivation below.
    let trusted_email = asserted.trusted_email();
    mapping.check_allowed(trusted_email)?;
    let desired_role = mapping.role_for(trusted_email, &asserted.groups);

    let existing = storage
        .get_identity_user(provider_key.to_string(), asserted.subject.clone())
        .await
        .map_err(SsoError::Internal)?;

    let Some((identity, user)) = existing else {
        return storage
            .create_sso_user(
                derive_username(asserted.email.as_deref(), provider_key, &asserted.subject),
                asserted.email.clone(),
                desired_role,
                NewIdentity {
                    provider: provider_key.to_string(),
                    kind,
                    subject: asserted.subject.clone(),
                    email: asserted.email.clone(),
                    display_name: asserted.display_name.clone(),
                },
            )
            .await
            .map_err(SsoError::Internal);
    };

    if user.disabled {
        return Err(SsoError::UserDisabled);
    }

    storage
        .touch_identity(
            identity.id,
            asserted.email.clone(),
            asserted.display_name.clone(),
        )
        .await
        .map_err(SsoError::Internal)?;

    if desired_role != user.role {
        match storage
            .update_role_and_disabled(user.id.clone(), Some(desired_role), None)
            .await
            .map_err(SsoError::Internal)?
        {
            UpdateUserOutcome::Updated => {}
            UpdateUserOutcome::LastAdmin => {
                tracing::warn!(
                    username = %user.username,
                    provider = provider_key,
                    "IdP mapping would demote the last enabled admin; keeping admin role"
                );
            }
            UpdateUserOutcome::NotFound => {
                return Err(SsoError::Internal(anyhow::anyhow!(
                    "user vanished during SSO role sync"
                )))
            }
        }
    }

    storage
        .get_user_by_id(user.id)
        .await
        .map_err(SsoError::Internal)?
        .ok_or_else(|| SsoError::Internal(anyhow::anyhow!("user vanished during SSO login")))
}

/// The JIT username: the email's local part sanitized to `[a-z0-9._-]`, else
/// `<provider-name>-<subject prefix>`. Collisions are resolved in storage.
pub(crate) fn derive_username(email: Option<&str>, provider_key: &str, subject: &str) -> String {
    if let Some(local) = email.and_then(|e| e.split('@').next()) {
        let sanitized = sanitize(local);
        if !sanitized.is_empty() {
            return sanitized;
        }
    }
    let provider_name = provider_key.split(':').next_back().unwrap_or(provider_key);
    let subject_prefix: String = sanitize(subject).chars().take(8).collect();
    if subject_prefix.is_empty() {
        provider_name.to_string()
    } else {
        format!("{provider_name}-{subject_prefix}")
    }
}

fn sanitize(raw: &str) -> String {
    raw.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_prefers_sanitized_email_local_part() {
        assert_eq!(
            derive_username(Some("Jon.Fuller+ci@example.com"), "oidc:google", "sub123"),
            "jon.fullerci"
        );
        assert_eq!(
            derive_username(Some("dev@example.com"), "saml:okta", "sub"),
            "dev"
        );
    }

    #[test]
    fn username_falls_back_to_provider_and_subject() {
        assert_eq!(
            derive_username(None, "oidc:google", "1234567890abc"),
            "google-12345678"
        );
        // A local part that sanitizes to nothing also falls back.
        assert_eq!(
            derive_username(Some("@example.com"), "saml:okta", "ABCDEF"),
            "okta-abcdef"
        );
        // Even a garbage subject yields something usable.
        assert_eq!(derive_username(None, "oidc:google", "@@@"), "google");
    }
}
