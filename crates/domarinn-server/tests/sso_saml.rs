//! SAML login flow, end to end against an in-test IdP that mints and signs
//! real responses (samael's xmlsec crypto): SP-initiated round trip,
//! metadata endpoint, tampered signatures, wrong audience, expired
//! confirmations, IdP-initiated mode, and the replay cache.
//!
//! Compiled only with `--features saml` (needs libxmlsec1 at build time);
//! CI runs it in a container with the C toolchain installed.
#![cfg(feature = "saml")]

mod common;

use axum::http::StatusCode;
use axum::Router;
use common::*;
use domarinn_server::base64::engine::general_purpose::STANDARD as B64;
use domarinn_server::base64::Engine;
use domarinn_server::samael::attribute::{Attribute, AttributeValue};
use domarinn_server::samael::crypto::{CertificateDer, Crypto, CryptoProvider};
use domarinn_server::samael::idp::{CertificateParams, IdentityProvider, KeyType, Rsa};
use domarinn_server::samael::schema::{
    Assertion, AttributeStatement, AudienceRestriction, Conditions, Issuer, Response, Status,
    StatusCode as SamlStatusCode, Subject, SubjectConfirmation, SubjectConfirmationData,
    SubjectNameID,
};
use domarinn_server::samael::signature::Signature;
use domarinn_server::samael::traits::ToXml;
use domarinn_server::sso::parse_sso_settings;
use domarinn_server::{AuthMode, Settings};
use std::io::Read;

const PUBLIC_URL: &str = "http://app.test";
const IDP_ENTITY_ID: &str = "https://idp.test/saml";
const IDP_SSO_URL: &str = "https://idp.test/sso";
const SP_ENTITY_ID: &str = "http://app.test/api/v1/auth/saml/test/metadata";
const ACS_URL: &str = "http://app.test/api/v1/auth/saml/test/acs";
const EMAIL_NAME_ID_FORMAT: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";

struct TestIdp {
    idp: IdentityProvider,
    cert: CertificateDer,
    /// Guard keeping the metadata file alive for the app's lifetime.
    _metadata_file: tempfile::NamedTempFile,
    metadata_path: String,
}

fn test_idp() -> TestIdp {
    let idp = IdentityProvider::generate_new(KeyType::Rsa(Rsa::Rsa2048)).expect("idp key");
    let cert = idp
        .create_certificate(&CertificateParams {
            common_name: "test-idp",
            issuer_name: "test-idp",
            days_until_expiration: 1,
        })
        .expect("idp cert");

    let cert_b64 = B64.encode(cert.der_data());
    let metadata = format!(
        r#"<?xml version="1.0"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{IDP_ENTITY_ID}">
  <md:IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="signing">
      <ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
        <ds:X509Data><ds:X509Certificate>{cert_b64}</ds:X509Certificate></ds:X509Data>
      </ds:KeyInfo>
    </md:KeyDescriptor>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="{IDP_SSO_URL}"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#
    );
    let file = tempfile::NamedTempFile::new().expect("metadata tempfile");
    std::fs::write(file.path(), metadata).expect("write metadata");
    let metadata_path = file.path().to_string_lossy().to_string();
    TestIdp {
        idp,
        cert,
        metadata_path,
        _metadata_file: file,
    }
}

async fn saml_app(idp: &TestIdp, extra: &[(&str, &str)]) -> (Router, tempfile::TempDir) {
    let mut vars: Vec<(String, String)> = vec![
        ("DOMARINN_SAML_PROVIDERS".into(), "test".into()),
        (
            "DOMARINN_SAML_TEST_IDP_METADATA_FILE".into(),
            idp.metadata_path.clone(),
        ),
    ];
    vars.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let sso = parse_sso_settings(&vars.into_iter().collect()).expect("sso settings");
    let settings = Settings {
        public_url: Some(PUBLIC_URL.to_string()),
        sso,
        ..Default::default()
    };
    test_app_with_mode(settings, AuthMode::Closed).await
}

fn location(reply: &Reply) -> String {
    reply
        .headers
        .get("location")
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string()
}

fn set_cookie_value(reply: &Reply, name: &str) -> Option<String> {
    reply.set_cookies().iter().find_map(|c| {
        c.strip_prefix(&format!("{name}="))
            .map(|rest| rest.split(';').next().unwrap_or("").to_string())
    })
}

struct StartedLogin {
    relay_state: String,
    request_id: String,
}

/// Drive the start endpoint; decode the deflated SAMLRequest to recover the
/// AuthnRequest id the IdP must echo as InResponseTo.
async fn start_login(app: &Router, return_to: Option<&str>) -> StartedLogin {
    let uri = match return_to {
        Some(r) => format!("/api/v1/auth/saml/test/start?return_to={r}"),
        None => "/api/v1/auth/saml/test/start".to_string(),
    };
    let reply = get(app, &uri).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let redirect = location(&reply);
    assert!(redirect.starts_with(IDP_SSO_URL), "{redirect}");

    let parsed = url::Url::parse(&redirect).unwrap();
    let saml_request = parsed
        .query_pairs()
        .find(|(k, _)| k == "SAMLRequest")
        .map(|(_, v)| v.to_string())
        .expect("SAMLRequest param");
    let relay_state = parsed
        .query_pairs()
        .find(|(k, _)| k == "RelayState")
        .map(|(_, v)| v.to_string())
        .expect("RelayState param");

    // The browser-binding cookie is set and carries the RelayState.
    assert_eq!(
        set_cookie_value(&reply, "domarinn_saml_txn").as_deref(),
        Some(relay_state.as_str())
    );

    let compressed = B64.decode(saml_request).expect("SAMLRequest b64");
    let mut xml = String::new();
    flate2::read::DeflateDecoder::new(compressed.as_slice())
        .read_to_string(&mut xml)
        .expect("SAMLRequest inflate");
    let request_id = xml
        .split("ID=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("AuthnRequest ID")
        .to_string();

    StartedLogin {
        relay_state,
        request_id,
    }
}

struct ResponseSpec<'a> {
    name_id: &'a str,
    name_id_format: &'a str,
    in_response_to: &'a str,
    audience: &'a str,
    recipient: &'a str,
    groups: Vec<&'a str>,
    /// Offset (seconds from now) for the confirmation/conditions expiry.
    expires_in_secs: i64,
}

impl<'a> ResponseSpec<'a> {
    fn ok(login: &'a StartedLogin, email: &'a str) -> ResponseSpec<'a> {
        ResponseSpec {
            name_id: email,
            name_id_format: EMAIL_NAME_ID_FORMAT,
            in_response_to: &login.request_id,
            audience: SP_ENTITY_ID,
            recipient: ACS_URL,
            groups: vec!["admins", "devs"],
            expires_in_secs: 300,
        }
    }
}

/// Build and xmlsec-sign a SAML response. Hand-rolled (rather than samael's
/// `sign_authn_response`) because the SP validation requires a
/// `SubjectConfirmationData/@NotOnOrAfter`, which samael's own template
/// omits.
fn signed_response_b64(idp: &TestIdp, spec: &ResponseSpec) -> String {
    let now = chrono::Utc::now();
    let expiry = now + chrono::Duration::seconds(spec.expires_in_secs);
    let issuer = Issuer {
        value: Some(IDP_ENTITY_ID.to_string()),
        ..Default::default()
    };
    let response_id = format!("_r{}", ulid::Ulid::generate().to_string().to_lowercase());
    let assertion_id = format!("_a{}", ulid::Ulid::generate().to_string().to_lowercase());

    let assertion = Assertion {
        id: assertion_id,
        issue_instant: now,
        version: "2.0".to_string(),
        issuer: issuer.clone(),
        signature: None,
        subject: Some(Subject {
            name_id: Some(SubjectNameID {
                format: Some(spec.name_id_format.to_string()),
                value: spec.name_id.to_string(),
            }),
            subject_confirmations: Some(vec![SubjectConfirmation {
                method: Some("urn:oasis:names:tc:SAML:2.0:cm:bearer".to_string()),
                name_id: None,
                subject_confirmation_data: Some(SubjectConfirmationData {
                    not_before: None,
                    not_on_or_after: Some(expiry),
                    recipient: Some(spec.recipient.to_string()),
                    in_response_to: Some(spec.in_response_to.to_string()),
                    address: None,
                    content: None,
                }),
            }]),
        }),
        conditions: Some(Conditions {
            not_before: None,
            not_on_or_after: Some(expiry),
            audience_restrictions: Some(vec![AudienceRestriction {
                audience: vec![spec.audience.to_string()],
            }]),
            one_time_use: None,
            proxy_restriction: None,
        }),
        authn_statements: None,
        attribute_statements: Some(vec![AttributeStatement {
            attributes: vec![
                Attribute {
                    friendly_name: None,
                    name: Some("groups".to_string()),
                    name_format: None,
                    values: spec
                        .groups
                        .iter()
                        .map(|g| AttributeValue {
                            attribute_type: Some("xs:string".to_string()),
                            value: Some(g.to_string()),
                        })
                        .collect(),
                },
                Attribute {
                    friendly_name: None,
                    name: Some("displayName".to_string()),
                    name_format: None,
                    values: vec![AttributeValue {
                        attribute_type: Some("xs:string".to_string()),
                        value: Some("Sam L. User".to_string()),
                    }],
                },
            ],
        }]),
    };

    let response = Response {
        id: response_id.clone(),
        in_response_to: Some(spec.in_response_to.to_string()),
        version: "2.0".to_string(),
        issue_instant: now,
        destination: Some(spec.recipient.to_string()),
        consent: None,
        issuer: Some(issuer),
        signature: Some(Signature::template(&response_id, &idp.cert)),
        status: Some(Status {
            status_code: SamlStatusCode {
                value: Some("urn:oasis:names:tc:SAML:2.0:status:Success".to_string()),
            },
            status_message: None,
            status_detail: None,
        }),
        encrypted_assertion: None,
        assertion: Some(assertion),
    };

    // samael's Signature::template hardcodes SHA-1 digests; real IdPs (and
    // therefore our SP's algorithm allowlist) use SHA-256. Upgrade the
    // template's references before signing.
    let mut response = response;
    if let Some(signature) = response.signature.as_mut() {
        for reference in &mut signature.signed_info.reference {
            reference.digest_method.algorithm =
                domarinn_server::samael::signature::DigestAlgorithm::Sha256;
        }
    }

    let unsigned_xml = response.to_string().expect("serialize response");
    let key_der = idp.idp.export_private_key_der().expect("idp key der");
    let signed_xml = Crypto::sign_xml(unsigned_xml.as_str(), key_der.as_slice()).expect("sign");
    B64.encode(signed_xml.as_bytes())
}

/// POST to the ACS with the browser-binding cookie matching the RelayState
/// (the normal SP-initiated case).
async fn post_acs(app: &Router, b64: &str, relay_state: Option<&str>) -> Reply {
    let cookie = relay_state.map(|r| format!("domarinn_saml_txn={r}"));
    post_acs_with_cookie(app, b64, relay_state, cookie.as_deref()).await
}

/// POST to the ACS with an explicit binding cookie (or none), to exercise the
/// session-swap defense.
async fn post_acs_with_cookie(
    app: &Router,
    b64: &str,
    relay_state: Option<&str>,
    binding_cookie: Option<&str>,
) -> Reply {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("SAMLResponse", b64);
    if let Some(relay) = relay_state {
        form.append_pair("RelayState", relay);
    }
    let mut headers: Vec<(&str, &str)> =
        vec![("content-type", "application/x-www-form-urlencoded")];
    if let Some(cookie) = binding_cookie {
        headers.push(("cookie", cookie));
    }
    send_with_headers(
        app,
        "POST",
        "/api/v1/auth/saml/test/acs",
        &headers,
        form.finish().into_bytes(),
    )
    .await
}

#[tokio::test]
async fn sp_initiated_round_trip_provisions_admin() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[("DOMARINN_SAML_TEST_ADMIN_GROUPS", "admins")]).await;

    let login = start_login(&app, Some("/cache")).await;
    let b64 = signed_response_b64(&idp, &ResponseSpec::ok(&login, "user@example.test"));

    let reply = post_acs(&app, &b64, Some(&login.relay_state)).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{:?}", reply.json());
    assert_eq!(location(&reply), "/cache");

    let session = set_cookie_value(&reply, "domarinn_session").expect("session cookie");
    let cookie = format!("domarinn_session={session}");
    let me = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie)],
        vec![],
    )
    .await;
    assert_eq!(me.json()["authenticated"], true);
    assert_eq!(me.json()["user"]["username"], "user");
    assert_eq!(me.json()["user"]["role"], "admin");

    // Same NameID again -> same account.
    let login2 = start_login(&app, None).await;
    let b64_2 = signed_response_b64(&idp, &ResponseSpec::ok(&login2, "user@example.test"));
    let reply2 = post_acs(&app, &b64_2, Some(&login2.relay_state)).await;
    assert_eq!(location(&reply2), "/");
    let users =
        send_with_headers(&app, "GET", "/api/v1/users", &[("cookie", &cookie)], vec![]).await;
    assert_eq!(users.json()["users"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn sp_metadata_endpoint_serves_entity_descriptor() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[]).await;

    let reply = get(&app, "/api/v1/auth/saml/test/metadata").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.headers.get("content-type").unwrap(),
        "application/samlmetadata+xml"
    );
    let xml = String::from_utf8(reply.body.clone()).unwrap();
    assert!(xml.contains(SP_ENTITY_ID), "{xml}");
    assert!(xml.contains(ACS_URL), "{xml}");
}

#[tokio::test]
async fn tampered_response_is_rejected() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[]).await;

    let login = start_login(&app, None).await;
    let b64 = signed_response_b64(&idp, &ResponseSpec::ok(&login, "user@example.test"));

    // Flip the asserted NameID after signing.
    let xml = String::from_utf8(B64.decode(&b64).unwrap()).unwrap();
    let tampered = B64.encode(xml.replace("user@example.test", "admin@example.test"));
    let reply = post_acs(&app, &tampered, Some(&login.relay_state)).await;
    assert!(
        location(&reply).starts_with("/login?sso_error=provider_error"),
        "{}",
        location(&reply)
    );
}

#[tokio::test]
async fn wrong_audience_and_expired_confirmation_are_rejected() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[]).await;

    let login = start_login(&app, None).await;
    let mut spec = ResponseSpec::ok(&login, "user@example.test");
    spec.audience = "https://someone-else.example/sp";
    let reply = post_acs(
        &app,
        &signed_response_b64(&idp, &spec),
        Some(&login.relay_state),
    )
    .await;
    assert!(location(&reply).starts_with("/login?sso_error=provider_error"));

    let login2 = start_login(&app, None).await;
    let mut spec2 = ResponseSpec::ok(&login2, "user@example.test");
    spec2.expires_in_secs = -3600; // well past any skew
    let reply2 = post_acs(
        &app,
        &signed_response_b64(&idp, &spec2),
        Some(&login2.relay_state),
    )
    .await;
    assert!(location(&reply2).starts_with("/login?sso_error=provider_error"));
}

#[tokio::test]
async fn unknown_relay_state_is_rejected_when_sp_initiated_only() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[]).await;

    let login = start_login(&app, None).await;
    let b64 = signed_response_b64(&idp, &ResponseSpec::ok(&login, "user@example.test"));

    // Unknown relay -> no transaction -> rejected.
    let unknown = post_acs(&app, &b64, Some("not-a-real-relay-state")).await;
    assert!(location(&unknown).starts_with("/login?sso_error=invalid_state"));

    // Missing relay entirely -> rejected (IdP-initiated is off by default).
    let missing = post_acs(&app, &b64, None).await;
    assert!(location(&missing).starts_with("/login?sso_error=invalid_state"));
}

#[tokio::test]
async fn session_swap_without_the_binding_cookie_is_rejected() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[]).await;

    // A valid, signed response for a real SP-initiated transaction...
    let login = start_login(&app, None).await;
    let b64 = signed_response_b64(&idp, &ResponseSpec::ok(&login, "user@example.test"));

    // ...replayed into a browser that lacks the binding cookie (login-CSRF /
    // session-swap) is rejected even though the RelayState + signature are
    // valid.
    let no_cookie = post_acs_with_cookie(&app, &b64, Some(&login.relay_state), None).await;
    assert!(location(&no_cookie).starts_with("/login?sso_error=invalid_state"));

    // A mismatched cookie (the attacker's own txn) is likewise rejected.
    let wrong_cookie = post_acs_with_cookie(
        &app,
        &b64,
        Some(&login.relay_state),
        Some("domarinn_saml_txn=someone-else"),
    )
    .await;
    assert!(location(&wrong_cookie).starts_with("/login?sso_error=invalid_state"));

    // The legitimate browser (matching cookie) still succeeds.
    let ok = post_acs(&app, &b64, Some(&login.relay_state)).await;
    assert_eq!(ok.status, StatusCode::SEE_OTHER, "{:?}", ok.json());
}

#[tokio::test]
async fn idp_initiated_mode_works_and_replays_are_blocked() {
    let idp = test_idp();
    let (app, _dir) = saml_app(&idp, &[("DOMARINN_SAML_TEST_ALLOW_IDP_INITIATED", "true")]).await;

    // Unsolicited response: no transaction, no RelayState.
    let fake_login = StartedLogin {
        relay_state: String::new(),
        request_id: "_unsolicited".to_string(),
    };
    let b64 = signed_response_b64(&idp, &ResponseSpec::ok(&fake_login, "user@example.test"));
    let first = post_acs(&app, &b64, None).await;
    assert_eq!(first.status, StatusCode::SEE_OTHER, "{:?}", first.json());
    assert_eq!(location(&first), "/");
    assert!(set_cookie_value(&first, "domarinn_session").is_some());

    // The exact same assertion again -> replay cache blocks it.
    let replay = post_acs(&app, &b64, None).await;
    assert!(
        location(&replay).starts_with("/login?sso_error=replayed"),
        "{}",
        location(&replay)
    );
}
