//! Reusable contract suite for `LlmProvider` adapters. Each `llm-*` crate runs it against a mock
//! HTTP server speaking that provider's wire format, so every adapter is held to the same
//! behaviour: normalized streaming, error mapping, model listing, early cancellation and no
//! key leakage.
//!
//! The suite panics on failure (call it from a test).

use std::sync::Arc;

use application::ports::{LlmChunk, LlmError, LlmProvider, LlmRequest, Usage};
use async_trait::async_trait;
use domain::{ChatMessage, ModelId};
use futures::StreamExt;

/// What the mock server should do for the next calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmScenario {
    /// A successful reply made of these text deltas, with this token usage.
    Success { chunks: Vec<String>, usage: Usage },
    /// The server rejects the credentials (HTTP 401).
    Unauthorized,
    /// The server asks the client to slow down (HTTP 429).
    RateLimited,
    /// The server fails (HTTP 5xx).
    ServerError,
    /// HTTP 200 whose body is not in the provider's format.
    MalformedBody,
    /// The model catalogue contains exactly these ids.
    Models(Vec<String>),
}

/// Adapter-specific setup: mock the server for a scenario and return a provider that talks to it.
#[async_trait]
pub trait LlmContractHarness: Send + Sync {
    async fn provider_for(&self, scenario: &LlmScenario) -> Arc<dyn LlmProvider>;

    /// The API key the provider was configured with, if it uses one. The suite checks that no
    /// error message ever contains it.
    fn api_key(&self) -> Option<&str> {
        None
    }
}

fn request() -> LlmRequest {
    LlmRequest {
        model: ModelId::new("contract-model"),
        system: Some("You are a test.".into()),
        messages: vec![ChatMessage::user("say hello")],
        max_tokens: Some(64),
        temperature: Some(0.0),
    }
}

fn assert_no_key(h: &dyn LlmContractHarness, what: &str, err: &LlmError) {
    if let Some(key) = h.api_key() {
        let text = format!("{err} {err:?}");
        assert!(
            !text.contains(key),
            "contract: {what} leaked the API key: {text}"
        );
    }
}

pub async fn run_llm_provider_contract(h: &dyn LlmContractHarness) {
    streaming_success(h).await;
    complete_matches_stream(h).await;
    early_drop_is_safe(h).await;
    errors(h).await;
    models(h).await;
}

fn success() -> LlmScenario {
    LlmScenario::Success {
        chunks: vec!["Hello".into(), ", ".into(), "wörld".into()],
        usage: Usage {
            input_tokens: 11,
            output_tokens: 7,
        },
    }
}

async fn streaming_success(h: &dyn LlmContractHarness) {
    let provider = h.provider_for(&success()).await;
    let items: Vec<_> = provider.stream(request()).collect().await;

    let mut text = String::new();
    let mut usage = None;
    let mut done = 0;
    for (i, item) in items.iter().enumerate() {
        match item {
            Ok(LlmChunk::Text(t)) => {
                assert_eq!(done, 0, "contract: text after Done");
                text.push_str(t);
            }
            Ok(LlmChunk::Usage(u)) => {
                assert!(
                    usage.replace(*u).is_none(),
                    "contract: more than one Usage chunk"
                );
            }
            Ok(LlmChunk::Done) => {
                done += 1;
                assert_eq!(i, items.len() - 1, "contract: Done must be last");
            }
            Err(e) => panic!("contract: unexpected stream error: {e:?}"),
        }
    }
    assert_eq!(text, "Hello, wörld", "contract: concatenated text");
    assert_eq!(done, 1, "contract: exactly one Done");
    assert_eq!(
        usage,
        Some(Usage {
            input_tokens: 11,
            output_tokens: 7
        }),
        "contract: usage"
    );
}

async fn complete_matches_stream(h: &dyn LlmContractHarness) {
    let provider = h.provider_for(&success()).await;
    match provider.complete(request()).await {
        Ok(r) => {
            assert_eq!(r.text, "Hello, wörld", "contract: complete text");
            assert_eq!(
                r.usage,
                Some(Usage {
                    input_tokens: 11,
                    output_tokens: 7
                })
            );
        }
        Err(e) => panic!("contract: complete failed: {e:?}"),
    }
}

async fn early_drop_is_safe(h: &dyn LlmContractHarness) {
    let provider = h.provider_for(&success()).await;
    let mut stream = provider.stream(request());
    let first = stream.next().await;
    assert!(
        matches!(first, Some(Ok(LlmChunk::Text(_)))),
        "contract: first item {first:?}"
    );
    drop(stream); // cancelling must not panic or hang
    // The provider is still usable afterwards.
    let again = h.provider_for(&success()).await;
    assert!(again.complete(request()).await.is_ok());
}

async fn errors(h: &dyn LlmContractHarness) {
    for (scenario, expect) in [
        (LlmScenario::Unauthorized, "Authentication"),
        (LlmScenario::RateLimited, "RateLimited"),
        (LlmScenario::ServerError, "Provider"),
        (LlmScenario::MalformedBody, "Malformed"),
    ] {
        let provider = h.provider_for(&scenario).await;

        let via_complete = provider.complete(request()).await.err();
        let mut stream = provider.stream(request());
        let mut via_stream = None;
        while let Some(item) = stream.next().await {
            if let Err(e) = item {
                via_stream = Some(e);
                break;
            }
        }
        for (how, err) in [("complete", via_complete), ("stream", via_stream)] {
            let Some(err) = err else {
                panic!("contract: {scenario:?} via {how} gave no error")
            };
            let kind = match &err {
                LlmError::Authentication => "Authentication",
                LlmError::RateLimited => "RateLimited",
                LlmError::Provider(_) => "Provider",
                LlmError::Malformed(_) => "Malformed",
                LlmError::Network(_) => "Network",
                LlmError::Cancelled => "Cancelled",
            };
            assert_eq!(
                kind, expect,
                "contract: {scenario:?} via {how} gave {err:?}"
            );
            assert_no_key(h, &format!("{scenario:?}"), &err);
        }
    }
}

async fn models(h: &dyn LlmContractHarness) {
    let provider = h
        .provider_for(&LlmScenario::Models(vec!["alpha".into(), "beta".into()]))
        .await;
    match provider.list_models().await {
        Ok(models) => {
            let ids: Vec<_> = models.iter().map(|m| m.id.as_str().to_owned()).collect();
            assert_eq!(ids, ["alpha", "beta"], "contract: model ids");
        }
        Err(e) => panic!("contract: list_models failed: {e:?}"),
    }
    let bad = h.provider_for(&LlmScenario::Unauthorized).await;
    match bad.list_models().await.err() {
        Some(LlmError::Authentication) => {}
        other => panic!("contract: list_models with bad credentials gave {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FakeLlm;

    /// The fake provider must itself satisfy the contract the adapters are held to.
    struct FakeHarness;

    #[async_trait]
    impl LlmContractHarness for FakeHarness {
        async fn provider_for(&self, scenario: &LlmScenario) -> Arc<dyn LlmProvider> {
            let fake = FakeLlm::new("fake", &[]);
            Arc::new(match scenario {
                LlmScenario::Success { chunks, usage } => {
                    let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
                    FakeLlm::new("fake", &refs).with_usage(*usage)
                }
                LlmScenario::Unauthorized => fake.failing(LlmError::Authentication),
                LlmScenario::RateLimited => fake.failing(LlmError::RateLimited),
                LlmScenario::ServerError => fake.failing(LlmError::Provider("500".into())),
                LlmScenario::MalformedBody => fake.failing(LlmError::Malformed("bad".into())),
                LlmScenario::Models(ids) => {
                    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                    fake.with_models(&refs)
                }
            })
        }
    }

    #[test]
    fn the_fake_provider_passes_its_own_contract() {
        futures::executor::block_on(run_llm_provider_contract(&FakeHarness));
    }

    /// A provider that breaks the rules must be caught by the suite.
    struct LeakyHarness;

    #[async_trait]
    impl LlmContractHarness for LeakyHarness {
        async fn provider_for(&self, scenario: &LlmScenario) -> Arc<dyn LlmProvider> {
            match scenario {
                LlmScenario::Unauthorized => Arc::new(
                    FakeLlm::new("fake", &[])
                        .failing(LlmError::Provider("key sk-123 rejected".into())),
                ),
                other => FakeHarness.provider_for(other).await,
            }
        }
        fn api_key(&self) -> Option<&str> {
            Some("sk-123")
        }
    }

    #[test]
    #[should_panic(expected = "contract")]
    fn the_suite_fails_a_provider_that_maps_errors_wrongly_or_leaks_keys() {
        futures::executor::block_on(run_llm_provider_contract(&LeakyHarness));
    }
}
