//! IdP claim/attribute → domarinn authorization mapping.
//!
//! Pure decisions over what the IdP asserted: whether the person may sign in
//! at all (email-domain allowlist) and which [`Role`] they get (admin groups
//! or admin emails). Group extraction itself is protocol-specific and lives
//! with each provider; this module only judges the extracted values.

use crate::domain::Role;
use crate::sso::SsoError;

/// Per-provider authorization mapping, parsed from the provider's
/// `_ADMIN_GROUPS` / `_ADMIN_EMAILS` / `_ALLOWED_EMAIL_DOMAINS` env vars.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleMapping {
    /// IdP group names whose members become admins (exact match — group
    /// names are case-sensitive identifiers at the IdP).
    pub admin_groups: Vec<String>,
    /// Emails that become admins regardless of groups — the escape hatch for
    /// IdPs that expose no groups claim (Google).
    pub admin_emails: Vec<String>,
    /// When non-empty, only emails in these domains may sign in.
    pub allowed_email_domains: Vec<String>,
    /// The role a login that matches no admin rule lands on. `None` keeps the
    /// historical [`Role::Member`]; an operator who wants SSO to provision
    /// read-only accounts sets [`Role::Viewer`] here.
    pub default_role: Option<Role>,
}

impl RoleMapping {
    /// Enforce the email-domain allowlist. An empty allowlist admits
    /// everyone; a configured allowlist rejects a missing email outright —
    /// we cannot prove an unknown address is in-domain.
    pub fn check_allowed(&self, email: Option<&str>) -> Result<(), SsoError> {
        if self.allowed_email_domains.is_empty() {
            return Ok(());
        }
        let Some(domain) = email.and_then(|e| e.rsplit_once('@')).map(|(_, d)| d) else {
            return Err(SsoError::EmailNotAllowed);
        };
        if self
            .allowed_email_domains
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(domain))
        {
            Ok(())
        } else {
            Err(SsoError::EmailNotAllowed)
        }
    }

    /// The role this login's claims earn, falling back to [`default_role`].
    /// Re-evaluated on every SSO login so the IdP stays the source of truth
    /// for SSO-provisioned users.
    ///
    /// [`default_role`]: RoleMapping::default_role
    pub fn role_for(&self, email: Option<&str>, groups: &[String]) -> Role {
        let admin_by_group = groups
            .iter()
            .any(|g| self.admin_groups.iter().any(|admin| admin == g));
        let admin_by_email = email.is_some_and(|e| {
            self.admin_emails
                .iter()
                .any(|admin| admin.eq_ignore_ascii_case(e))
        });
        if admin_by_group || admin_by_email {
            Role::Admin
        } else {
            self.default_role.unwrap_or(Role::Member)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> RoleMapping {
        RoleMapping {
            admin_groups: vec!["sso-admins".into()],
            admin_emails: vec!["ops@example.com".into()],
            allowed_email_domains: vec!["example.com".into()],
            default_role: None,
        }
    }

    #[test]
    fn empty_allowlist_admits_everyone_even_without_email() {
        let open = RoleMapping::default();
        assert!(open.check_allowed(None).is_ok());
        assert!(open.check_allowed(Some("anyone@example.com")).is_ok());
    }

    #[test]
    fn domain_allowlist_is_case_insensitive_and_rejects_missing_email() {
        let m = mapping();
        assert!(m.check_allowed(Some("jon@example.com")).is_ok());
        assert!(m.check_allowed(Some("jon@EXAMPLE.COM")).is_ok());
        assert!(matches!(
            m.check_allowed(Some("jon@elsewhere.example")),
            Err(SsoError::EmailNotAllowed)
        ));
        assert!(matches!(
            m.check_allowed(None),
            Err(SsoError::EmailNotAllowed)
        ));
        // No `@` at all -> rejected, not a panic.
        assert!(matches!(
            m.check_allowed(Some("not-an-email")),
            Err(SsoError::EmailNotAllowed)
        ));
    }

    #[test]
    fn role_mapping_truth_table() {
        let m = mapping();
        // Group membership -> admin (exact, case-sensitive).
        assert_eq!(
            m.role_for(Some("a@example.com"), &["sso-admins".into()]),
            Role::Admin
        );
        assert_eq!(
            m.role_for(None, &["SSO-Admins".into()]),
            Role::Member,
            "group names match exactly"
        );
        // Admin email -> admin (case-insensitive), regardless of groups.
        assert_eq!(m.role_for(Some("OPS@example.com"), &[]), Role::Admin);
        // Neither -> member.
        assert_eq!(
            m.role_for(Some("dev@example.com"), &["devs".into()]),
            Role::Member
        );
        // No mapping configured at all -> everyone is a member.
        assert_eq!(
            RoleMapping::default().role_for(Some("x@y.z"), &["g".into()]),
            Role::Member
        );
    }

    /// An operator who wants SSO to provision read-only accounts sets the
    /// default role; the admin rules keep overriding it.
    #[test]
    fn a_configured_default_role_replaces_member_but_never_beats_admin() {
        let m = RoleMapping {
            default_role: Some(Role::Viewer),
            ..mapping()
        };
        assert_eq!(
            m.role_for(Some("dev@example.com"), &["devs".into()]),
            Role::Viewer
        );
        assert_eq!(
            m.role_for(Some("a@example.com"), &["sso-admins".into()]),
            Role::Admin
        );
        assert_eq!(m.role_for(Some("OPS@example.com"), &[]), Role::Admin);
    }
}
