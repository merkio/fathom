//! Runs the shared `LlmProvider` contract against a mock OpenAI-style server, and checks the
//! exact request the adapter sends. Fixtures follow the documented Chat Completions wire format.
#![allow(clippy::expect_used)] // test helpers

use std::sync::{Arc, Mutex};

use application::ports::{LlmError, LlmProvider, LlmRequest, SecretStore};
use async_trait::async_trait;
use domain::{ChatMessage, ModelId, Role, SecretRef};
use llm_openai::{MaxTokensParam, OpenAiConfig, OpenAiProvider};
use secrecy::SecretString;
use serde_json::{Value, json};
use test_support::InMemorySecrets;
use test_support::llm_contract::{LlmContractHarness, LlmScenario, run_llm_provider_contract};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-openai-key-5678";

fn success_body(chunks: &[String], prompt: u32, completion: u32) -> String {
    let mut out = String::new();
    let mut push = |v: Value| out.push_str(&format!("data: {v}\n\n"));
    push(
        json!({"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}),
    );
    for c in chunks {
        push(
            json!({"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":c},"finish_reason":null}]}),
        );
    }
    push(
        json!({"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
    );
    push(
        json!({"id":"c1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":prompt,"completion_tokens":completion,"total_tokens":prompt+completion}}),
    );
    out.push_str("data: [DONE]\n\n");
    out
}

fn event_stream(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn error_body(code: &str, message: &str) -> Value {
    json!({"error":{"message":message,"type":"invalid_request_error","code":code}})
}

struct Harness {
    servers: Mutex<Vec<MockServer>>,
    secrets: Arc<InMemorySecrets>,
}

impl Harness {
    async fn new() -> Self {
        let secrets = Arc::new(InMemorySecrets::default());
        secrets
            .set(&SecretRef::new("openai"), SecretString::from(KEY))
            .await
            .expect("store key");
        Self {
            servers: Mutex::new(Vec::new()),
            secrets,
        }
    }

    fn provider(&self, server: &MockServer) -> Arc<dyn LlmProvider> {
        let config = OpenAiConfig::new(SecretRef::new("openai"))
            .with_base_url(format!("{}/v1", server.uri()));
        Arc::new(OpenAiProvider::new(config, self.secrets.clone()).expect("provider"))
    }
}

#[async_trait]
impl LlmContractHarness for Harness {
    async fn provider_for(&self, scenario: &LlmScenario) -> Arc<dyn LlmProvider> {
        let server = MockServer::start().await;
        let chat = Mock::given(method("POST")).and(path("/v1/chat/completions"));
        let models = Mock::given(method("GET")).and(path("/v1/models"));
        match scenario {
            LlmScenario::Success { chunks, usage } => {
                chat.respond_with(event_stream(success_body(
                    chunks,
                    usage.input_tokens,
                    usage.output_tokens,
                )))
                .mount(&server)
                .await;
            }
            LlmScenario::Unauthorized => {
                // OpenAI echoes a (masked) key; a careless server could echo all of it.
                let body = error_body(
                    "invalid_api_key",
                    &format!("Incorrect API key provided: {KEY}"),
                );
                for m in [chat, models] {
                    m.respond_with(ResponseTemplate::new(401).set_body_json(body.clone()))
                        .mount(&server)
                        .await;
                }
            }
            LlmScenario::RateLimited => {
                chat.respond_with(
                    ResponseTemplate::new(429)
                        .set_body_json(error_body("rate_limit_exceeded", "slow down")),
                )
                .mount(&server)
                .await;
            }
            LlmScenario::ServerError => {
                chat.respond_with(
                    ResponseTemplate::new(500)
                        .set_body_json(error_body("server_error", &format!("boom (key {KEY})"))),
                )
                .mount(&server)
                .await;
            }
            LlmScenario::MalformedBody => {
                chat.respond_with(event_stream("data: this is not json\n\n".into()))
                    .mount(&server)
                    .await;
            }
            LlmScenario::Models(ids) => {
                let data: Vec<Value> = ids
                    .iter()
                    .map(|id| json!({"id":id,"object":"model","owned_by":"test"}))
                    .collect();
                models
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"object":"list","data":data})),
                    )
                    .mount(&server)
                    .await;
            }
        }
        let provider = self.provider(&server);
        self.servers.lock().expect("lock").push(server);
        provider
    }

    fn api_key(&self) -> Option<&str> {
        Some(KEY)
    }
}

#[tokio::test]
async fn openai_passes_the_provider_contract() {
    run_llm_provider_contract(&Harness::new().await).await;
}

fn request() -> LlmRequest {
    LlmRequest {
        model: ModelId::new("gpt-test"),
        system: Some("Be brief.".into()),
        messages: vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
            ChatMessage::new(Role::System, "late rule"),
            ChatMessage::user("sql please"),
        ],
        max_tokens: Some(256),
        temperature: Some(0.25),
    }
}

#[tokio::test]
async fn sends_the_documented_request_shape() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(event_stream(success_body(&["ok".into()], 1, 1)))
        .expect(1)
        .mount(&server)
        .await;

    h.provider(&server)
        .complete(request())
        .await
        .expect("reply");

    let received = server.received_requests().await.expect("recorded");
    let body: Value = serde_json::from_slice(&received[0].body).expect("json");
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["max_tokens"], 256);
    assert_eq!(body["temperature"], 0.25);
    assert!(body.get("max_completion_tokens").is_none());
    assert_eq!(
        body["messages"],
        json!([
            {"role":"system","content":"Be brief."},
            {"role":"user","content":"hi"},
            {"role":"assistant","content":"hello"},
            {"role":"system","content":"late rule"},
            {"role":"user","content":"sql please"},
        ])
    );
}

#[tokio::test]
async fn newer_models_can_use_max_completion_tokens() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(event_stream(success_body(&["ok".into()], 1, 1)))
        .mount(&server)
        .await;
    let config = OpenAiConfig::new(SecretRef::new("openai"))
        .with_base_url(format!("{}/v1", server.uri()))
        .with_max_tokens_param(MaxTokensParam::MaxCompletionTokens);
    let provider = OpenAiProvider::new(config, h.secrets.clone()).expect("provider");
    provider.complete(request()).await.expect("reply");
    let received = server.received_requests().await.expect("recorded");
    let body: Value = serde_json::from_slice(&received[0].body).expect("json");
    assert_eq!(body["max_completion_tokens"], 256);
    assert!(body.get("max_tokens").is_none());
}

#[tokio::test]
async fn compatible_servers_without_a_key_get_no_authorization_header_and_a_custom_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(event_stream(success_body(&["ok".into()], 1, 1)))
        .mount(&server)
        .await;
    let config = OpenAiConfig::compatible("my-vllm", format!("{}/v1", server.uri()), None);
    let provider =
        OpenAiProvider::new(config, Arc::new(InMemorySecrets::default())).expect("provider");
    assert_eq!(provider.id().as_str(), "my-vllm");
    provider.complete(request()).await.expect("reply");
    let received = server.received_requests().await.expect("recorded");
    assert!(received[0].headers.get("authorization").is_none());
}

#[tokio::test]
async fn a_configured_but_missing_key_is_an_authentication_error_and_nothing_is_sent() {
    let server = MockServer::start().await;
    let config =
        OpenAiConfig::new(SecretRef::new("absent")).with_base_url(format!("{}/v1", server.uri()));
    let provider =
        OpenAiProvider::new(config, Arc::new(InMemorySecrets::default())).expect("provider");
    assert_eq!(
        provider.complete(request()).await.err(),
        Some(LlmError::Authentication)
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty()
    );
}

#[tokio::test]
async fn connection_failures_are_network_errors_without_the_key() {
    let h = Harness::new().await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let config = OpenAiConfig::new(SecretRef::new("openai"))
        .with_base_url(format!("http://127.0.0.1:{port}/v1"));
    let provider = OpenAiProvider::new(config, h.secrets.clone()).expect("provider");
    let err = provider
        .complete(request())
        .await
        .expect_err("nothing listens");
    assert!(matches!(err, LlmError::Network(_)), "{err:?}");
    assert!(!format!("{err:?}").contains(KEY));
}
