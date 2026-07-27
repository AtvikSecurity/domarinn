# OIDC test fixtures

## `test-idp-signing-key.pem` is not a secret

It is a throwaway RSA key generated solely so the in-process mock identity
provider in [`../../common/mock_oidc.rs`](../../common/mock_oidc.rs) can mint
RS256 `id_token`s during integration tests. It signs nothing outside that mock,
it has never protected anything, and it is not deployed anywhere.

It is committed rather than generated per-run so the tests are deterministic and
run offline with no key-generation cost.

The banner you would normally expect at the top of a file like this is
deliberately absent: `CoreRsaPrivateSigningKey::from_pem` parses via
`pem-rfc7468`, which requires the `-----BEGIN-----` boundary to be the first
thing in the file. Any leading comment makes the fixture fail to parse.

**If a scanner flagged this file, that is the expected outcome.** GitHub secret
scanning will raise an alert on first push; dismiss it as "used in tests".
Contributors running [gitleaks](https://github.com/gitleaks/gitleaks) locally
are covered by the allowlist in [`/.gitleaks.toml`](../../../../../.gitleaks.toml).

Do not reuse this key for anything, and do not copy this pattern for material
that actually needs to stay secret.
