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
}

/// Build a provider trait object from its config.
pub fn build_provider(cfg: &ProviderCfg) -> Result<Box<dyn Provider>, FactoryError> {
    match &cfg.kind {
        ProviderKind::Exec {
            command,
            env,
            timeout_ms,
            cache_salt,
        } => Ok(Box::new(ExecProvider::new(
            cfg.id.clone(),
            command.clone(),
            env.clone(),
            *timeout_ms,
            cache_salt.clone(),
        ))),
        ProviderKind::Anthropic {
            model,
            base_url,
            api_key_env,
            params,
            pricing,
        } => Ok(Box::new(AnthropicProvider::new(
            cfg.id.clone(),
            model.clone(),
            base_url.clone(),
            api_key_env.clone(),
            params.clone(),
            pricing.as_deref().cloned(),
        ))),
        ProviderKind::Openai {
            model,
            base_url,
            api_key_env,
            params,
            pricing,
        } => Ok(Box::new(OpenAiProvider::new(
            cfg.id.clone(),
            model.clone(),
            base_url.clone(),
            api_key_env.clone(),
            params.clone(),
            pricing.as_deref().cloned(),
        ))),
        ProviderKind::Http {
            url,
            method,
            headers,
            body,
            output_expr,
        } => Ok(Box::new(HttpProvider::new(
            cfg.id.clone(),
            url.clone(),
            *method,
            headers.clone(),
            body.clone(),
            output_expr.clone(),
        ))),
        ProviderKind::Embeddings { .. } => Err(FactoryError::Unsupported {
            id: cfg.id.clone(),
            kind: "embeddings",
        }),
    }
}

/// Build the embeddings provider (for `similar` assertions) from the first
/// `type: embeddings` provider in the suite, if any.
pub fn build_embeddings(suite: &Suite) -> Option<EmbeddingsProvider> {
    suite.providers.iter().find_map(|p| match &p.kind {
        ProviderKind::Embeddings {
            model,
            base_url,
            api_key_env,
            params,
        } => Some(EmbeddingsProvider::new(
            model.clone(),
            base_url.clone(),
            api_key_env.clone(),
            params.clone(),
        )),
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
        let provider = build_provider(&suite.providers[0]).unwrap();
        assert_eq!(provider.id(), "p");
        assert!(
            provider.cacheable(),
            "exec is cached by default; the program's own identity is in the \
             fingerprint, so a rebuild busts the entry without a hand-set salt"
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
            assert!(build_provider(provider).is_ok(), "{}", provider.id);
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
        assert!(build_provider(&suite.providers[0]).is_err());
    }
}
