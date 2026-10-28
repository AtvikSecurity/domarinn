//! Building [`Provider`] trait objects from config.
//!
//! Supports `exec`, `anthropic`, `openai`, and `http`. `embeddings` providers
//! are used by the `similar` assertion and are constructed by the grader path.

use crate::anthropic::AnthropicProvider;
use crate::config::{Provider as ProviderCfg, ProviderKind, Suite};
use crate::embeddings::EmbeddingsProvider;
use crate::exec_provider::ExecProvider;
use crate::http_provider::HttpProvider;
use crate::openai::OpenAiProvider;
use crate::provider::Provider;

#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    #[error("provider '{id}': {kind} providers are not yet supported")]
    Unsupported { id: String, kind: &'static str },
    /// A `cache_salt: "$digest: <glob>"` that could not be resolved — the glob
    /// matched nothing, escaped the suite directory, or a matched file could not
    /// be read. Fatal rather than a fallback to the literal string: a salt that
    /// silently stops tracking its sources is a cache pin that lies.
    #[error("provider '{id}': resolving `cache_salt`: {source}")]
    DigestSalt {
        id: String,
        #[source]
        source: crate::resolve::ResolveError,
    },
    /// A `request:` block whose templates could not be rendered, or a malformed
    /// [`crate::request_cfg::GLOBAL_HEADERS_ENV`]. Fatal at build time so the
    /// whole run fails before the first case rather than on every case.
    #[error("provider '{id}': {source}")]
    Request {
        id: String,
        #[source]
        source: crate::request_cfg::RequestError,
    },
}

/// `with_timeout`, when there is one to apply.
///
/// A local trait over the four HTTP-backed kinds rather than an `Option`
/// parameter on each of their builders, so a provider constructed directly keeps
/// the plain `with_timeout(Duration)` and this file keeps one-line call sites.
trait WithTimeoutOpt: Sized {
    fn timeout(self, timeout: std::time::Duration) -> Self;
    fn with_timeout_opt(self, timeout: Option<std::time::Duration>) -> Self {
        match timeout {
            Some(t) => self.timeout(t),
            None => self,
        }
    }
}

impl WithTimeoutOpt for AnthropicProvider {
    fn timeout(self, timeout: std::time::Duration) -> Self {
        self.with_timeout(timeout)
    }
}
impl WithTimeoutOpt for OpenAiProvider {
    fn timeout(self, timeout: std::time::Duration) -> Self {
        self.with_timeout(timeout)
    }
}
impl WithTimeoutOpt for HttpProvider {
    fn timeout(self, timeout: std::time::Duration) -> Self {
        self.with_timeout(timeout)
    }
}
impl WithTimeoutOpt for EmbeddingsProvider {
    fn timeout(self, timeout: std::time::Duration) -> Self {
        self.with_timeout(timeout)
    }
}

/// Build a provider trait object from its config.
///
/// `base_dir` is the suite's directory — the cwd every `exec` child is spawned
/// in, and therefore what a relative `command` is resolved against when the
/// program's digest is taken. That digest is reported on a cache hit, never
/// keyed, so `base_dir` does not reach the cache key at all.
pub fn build_provider(
    cfg: &ProviderCfg,
    base_dir: Option<&std::path::Path>,
) -> Result<Box<dyn Provider>, FactoryError> {
    build_provider_with_timeout(cfg, base_dir, None)
}

/// [`build_provider`], with the suite's `runner.timeout_ms` applied.
///
/// `runner_timeout_ms` replaces each kind's built-in per-request default
/// (120 s for `anthropic`, `openai` and `http`; 60 s for `exec`). An `exec`
/// provider's own `timeout_ms` is more specific and still wins. `None` keeps
/// every default, which is exactly what [`build_provider`] does.
pub fn build_provider_with_timeout(
    cfg: &ProviderCfg,
    base_dir: Option<&std::path::Path>,
    runner_timeout_ms: Option<u64>,
) -> Result<Box<dyn Provider>, FactoryError> {
    let timeout = runner_timeout_ms.map(std::time::Duration::from_millis);
    match &cfg.kind {
        ProviderKind::Exec {
            command,
            env,
            timeout_ms,
            cache_salt,
        } => {
            // Resolve `$digest:` here rather than at load time: it reads the
            // filesystem, and the suite directory it must resolve against is a
            // property of this build, not of the parsed document.
            let cache_salt =
                crate::filevars::resolve_provider_digest_salt(cache_salt.as_deref(), base_dir)
                    .map_err(|source| FactoryError::DigestSalt {
                        id: cfg.id.clone(),
                        source,
                    })?;
            Ok(Box::new(ExecProvider::new(
                cfg.id.clone(),
                command.clone(),
                env.clone(),
                timeout_ms.or(runner_timeout_ms),
                cache_salt,
                base_dir,
            )))
        }
        ProviderKind::Anthropic {
            model,
            base_url,
            api_key_env,
            params,
            pricing,
            request,
            cache_salt,
        } => Ok(Box::new(
            AnthropicProvider::new(
                cfg.id.clone(),
                model.clone(),
                base_url.clone(),
                api_key_env.clone(),
                params.clone(),
                pricing.as_deref().cloned(),
            )
            .with_timeout_opt(timeout)
            .with_request(request.as_deref(), cache_salt.clone())
            .map_err(|source| FactoryError::Request {
                id: cfg.id.clone(),
                source,
            })?,
        )),
        ProviderKind::Openai {
            model,
            base_url,
            api_key_env,
            params,
            pricing,
            request,
            cache_salt,
        } => Ok(Box::new(
            OpenAiProvider::new(
                cfg.id.clone(),
                model.clone(),
                base_url.clone(),
                api_key_env.clone(),
                params.clone(),
                pricing.as_deref().cloned(),
            )
            .with_timeout_opt(timeout)
            .with_request(request.as_deref(), cache_salt.clone())
            .map_err(|source| FactoryError::Request {
                id: cfg.id.clone(),
                source,
            })?,
        )),
        ProviderKind::Http {
            url,
            method,
            headers,
            body,
            output_expr,
        } => Ok(Box::new(
            HttpProvider::new(
                cfg.id.clone(),
                url.clone(),
                *method,
                headers.clone(),
                body.clone(),
                output_expr.clone(),
            )
            .with_timeout_opt(timeout)
            .with_global_headers()
            .map_err(|source| FactoryError::Request {
                id: cfg.id.clone(),
                source,
            })?,
        )),
        ProviderKind::Embeddings { .. } => Err(FactoryError::Unsupported {
            id: cfg.id.clone(),
            kind: "embeddings",
        }),
    }
}

/// Build the embeddings provider (for `similar` assertions) from the first
/// `type: embeddings` provider in the suite, if any.
///
/// `runner.timeout_ms` applies here too: an embedding call is an HTTP request
/// to a provider the suite declared, and a suite that raised the ceiling for a
/// slow gateway means it for every call to that gateway.
pub fn build_embeddings(suite: &Suite) -> Option<EmbeddingsProvider> {
    let runner_timeout = suite
        .runner
        .as_ref()
        .and_then(|r| r.timeout_ms)
        .map(std::time::Duration::from_millis);
    suite.providers.iter().find_map(|p| match &p.kind {
        ProviderKind::Embeddings {
            model,
            base_url,
            api_key_env,
            params,
            pricing,
            request,
            cache_salt,
        } => EmbeddingsProvider::new(
            &p.id,
            model.clone(),
            base_url.clone(),
            api_key_env.clone(),
            params.clone(),
            pricing.as_deref(),
        )
        .with_timeout_opt(runner_timeout)
        .with_request(&p.id, request.as_deref(), cache_salt.clone())
        // A template that will not render is reported when the `similar`
        // assertion runs, where the grader can surface it: this function has no
        // error channel, and inventing one would put a `Result` on a path whose
        // only other failure mode is "the suite declares no embeddings
        // provider".
        .inspect_err(
            |e| tracing::error!(provider = %p.id, error = %e, "embeddings `request:` block"),
        )
        .ok(),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_an_exec_provider() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: p, type: exec, command: ["echo"]}]
"#,
        )
        .unwrap();
        let provider = build_provider(&suite.providers[0], None).unwrap();
        assert_eq!(provider.id(), "p");
        assert!(
            provider.cacheable(),
            "exec is cached like every other provider kind; `cache_salt` is a \
             version pin rather than the thing that enables caching"
        );
    }

    #[test]
    fn builds_network_providers() {
        let suite = crate::load_str(
            r#"
version: 1
providers:
  - {id: c, type: anthropic, model: m}
  - {id: g, type: openai, model: m}
  - {id: h, type: http, url: "http://x"}
"#,
        )
        .unwrap();
        for provider in &suite.providers {
            assert!(build_provider(provider, None).is_ok(), "{}", provider.id);
        }
    }

    #[test]
    fn embeddings_provider_is_not_a_direct_provider() {
        let suite = crate::load_str(
            r#"
version: 1
providers: [{id: e, type: embeddings, model: m}]
"#,
        )
        .unwrap();
        assert!(build_provider(&suite.providers[0], None).is_err());
    }

    /// A suite directory holding `src/a.rs` with the given contents.
    fn sut_tree(a: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), a).unwrap();
        dir
    }

    fn exec_suite_with_salt(salt: &str) -> crate::config::Suite {
        crate::load_str(&format!(
            "version: 1\nproviders: [{{id: p, type: exec, command: [\"./sut\"], cache_salt: \"{salt}\"}}]\n"
        ))
        .unwrap()
    }

    /// The docstring on `cache_salt` and the rebuild warning both name
    /// `$digest:`, but only *case* salts ever resolved it — a provider's went
    /// verbatim into the key, so the salt was the literal string and never moved
    /// when the sources did.
    #[test]
    fn a_provider_digest_salt_resolves_to_its_sources() {
        let dir = sut_tree("fn main() {}");
        let suite = exec_suite_with_salt("$digest: src/**/*.rs");
        let provider = build_provider(&suite.providers[0], Some(dir.path())).unwrap();

        let salt = provider.cache_salt().expect("a salt survives the build");
        assert_ne!(
            salt, "$digest: src/**/*.rs",
            "the literal spec must not reach the cache key"
        );

        // The whole point: editing a source moves the salt.
        std::fs::write(dir.path().join("src/a.rs"), "fn main() { todo!() }").unwrap();
        let rebuilt = build_provider(&suite.providers[0], Some(dir.path())).unwrap();
        assert_ne!(
            salt,
            rebuilt.cache_salt().unwrap(),
            "editing a matched source must change the salt"
        );
    }

    /// An ordinary salt is opt-in by prefix and passes through untouched, so a
    /// git SHA or release tag still keys exactly as written.
    #[test]
    fn an_opaque_provider_salt_passes_through_unchanged() {
        let dir = sut_tree("fn main() {}");
        let suite = exec_suite_with_salt("abc1234");
        let provider = build_provider(&suite.providers[0], Some(dir.path())).unwrap();
        assert_eq!(provider.cache_salt(), Some("abc1234"));
    }

    /// Same sandbox as every other file reference — a suite must not digest
    /// files outside its own tree.
    #[test]
    fn a_traversing_provider_digest_salt_is_rejected() {
        let dir = sut_tree("fn main() {}");
        let suite = exec_suite_with_salt("$digest: ../../../etc/*");
        assert!(build_provider(&suite.providers[0], Some(dir.path())).is_err());
    }

    /// An empty digest would be one constant salt shared by every such provider
    /// — no pin at all, wearing a hash.
    #[test]
    fn a_provider_digest_salt_matching_nothing_is_an_error() {
        let dir = sut_tree("fn main() {}");
        let suite = exec_suite_with_salt("$digest: src/missing-*.rs");
        let err = build_provider(&suite.providers[0], Some(dir.path()))
            .err()
            .expect("a glob matching nothing must fail the build");
        assert!(err.to_string().contains("matched no files"), "{err}");
    }

    /// Without a suite directory a relative glob would resolve against whatever
    /// cwd the caller happened to have — the machine-dependence that keeping the
    /// program's bytes out of the key exists to remove.
    #[test]
    fn a_provider_digest_salt_needs_a_suite_directory() {
        let suite = exec_suite_with_salt("$digest: src/**/*.rs");
        let err = build_provider(&suite.providers[0], None)
            .err()
            .expect("a `$digest:` salt without a suite dir must fail the build");
        assert!(err.to_string().contains("suite directory"), "{err}");
    }

    fn bare_request() -> crate::provider::ProviderRequest {
        crate::provider::ProviderRequest {
            tools: Vec::new(),
            prompt: None,
            vars: Default::default(),
            params: serde_json::Map::new(),
            test: Default::default(),
            case_salt: None,
        }
    }

    /// `runner.timeout_ms` replaces the HTTP client's 120 s default, and an
    /// expiry is a retriable `provider_timeout`.
    #[tokio::test]
    async fn the_runner_timeout_bounds_an_http_provider() {
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("late")
                    .set_delay(std::time::Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        let suite = crate::load_str(&format!(
            "version: 1\nproviders: [{{id: h, type: http, url: \"{}/x\"}}]\n",
            server.uri()
        ))
        .unwrap();
        let provider = build_provider_with_timeout(&suite.providers[0], None, Some(200)).unwrap();
        let started = std::time::Instant::now();
        let err = provider
            .call(&bare_request(), &Default::default())
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "the 120 s default applied instead of runner.timeout_ms"
        );
        assert_eq!(
            err.class().as_str(),
            crate::error_class::ErrorClass::PROVIDER_TIMEOUT
        );
        assert!(matches!(
            err,
            crate::provider::ProviderError::Retriable { .. }
        ));
    }

    /// An `exec` provider without its own `timeout_ms` takes the runner's.
    #[tokio::test]
    async fn an_exec_provider_inherits_the_runner_timeout() {
        let suite = crate::load_str(
            "version: 1\nproviders: [{id: p, type: exec, command: [sh, -c, \"sleep 5\"]}]\n",
        )
        .unwrap();
        let provider = build_provider_with_timeout(&suite.providers[0], None, Some(100)).unwrap();
        let err = provider
            .call(&bare_request(), &Default::default())
            .await
            .unwrap_err();
        assert_eq!(
            err.class().as_str(),
            crate::error_class::ErrorClass::PROVIDER_TIMEOUT
        );
    }

    /// ...but its own `timeout_ms` is more specific, and wins.
    #[tokio::test]
    async fn an_exec_providers_own_timeout_beats_the_runners() {
        let suite = crate::load_str(
            r#"
version: 1
providers:
  - id: p
    type: exec
    timeout_ms: 10000
    command: ["sh", "-c", "cat >/dev/null; sleep 0.3; printf '{\"output\":\"ok\"}'"]
"#,
        )
        .unwrap();
        let provider = build_provider_with_timeout(&suite.providers[0], None, Some(50)).unwrap();
        let resp = provider
            .call(&bare_request(), &Default::default())
            .await
            .expect("the provider's 10 s applies, not the runner's 50 ms");
        assert_eq!(resp.output, crate::types::Output::Text("ok".into()));
    }
}
