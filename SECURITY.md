# Security Policy

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report it privately through GitHub's [private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing/privately-reporting-a-security-vulnerability): go to the **Security** tab of this repository and click **Report a vulnerability**.

If that is unavailable to you, email **security@atviksecurity.com**.

A maintainer will triage the report. If it is accepted we will publish a security advisory and continue the conversation there, crediting you unless you would rather stay anonymous.

## What is in scope

domarinn runs evaluation suites and hosts a results server, so the interesting attack surface is roughly:

- **The results server** — authentication and session handling, the SSO (OIDC/SAML) flows, role and email-domain mapping, API authorization, and anything that lets one account read or modify another's runs.
- **Suite loading** — a malicious `domarinn.yaml` or template reaching beyond what the config author should be able to do (path traversal, environment disclosure, unintended process execution).
- **Provider and cache handling** — credential leakage into logs, cache entries, shared runs, or error output.

## What is not a vulnerability

- **`exec` providers run programs.** That is the entire point of the feature. Running an untrusted suite is equivalent to running untrusted code; treat a `domarinn.yaml` with the same suspicion as a shell script.
- **A model producing wrong, offensive, or unsafe output.** That is a finding for your eval suite to catch, which is what domarinn is for.
- **`crates/domarinn-server/tests/fixtures/oidc/test-idp-signing-key.pem`.** It is a throwaway key for an in-process mock identity provider, committed deliberately so tests are deterministic and offline. See the [README beside it](./crates/domarinn-server/tests/fixtures/oidc/README.md). Automated scanners flag it; it protects nothing.

## Supported versions

domarinn is pre-`1.0` and moves fast. Only the latest release is supported — fixes ship in a new release rather than as backports.
