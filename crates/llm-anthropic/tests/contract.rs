//! Runs the shared `LlmProvider` contract against a mock Anthropic server, and checks the
//! exact request the adapter sends. Fixtures follow the documented Messages API wire format.
#![allow(clippy::expect_used)] // test helpers

use std::sync::{Arc, Mutex};

use application::ports::{LlmProvider, LlmRequest, SecretStore};
use async_trait::async_trait;
use domain::{ChatMessage, ModelId, Role, SecretRef};
use llm_anthropic::{AnthropicConfig, AnthropicProvider};
use secrecy::SecretString;
use serde_json::{Value, json};
use test_support::InMemorySecrets;
use test_support::llm_contract::{LlmContractHarness, LlmScenario, run_llm_provider_contract};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-ant-test-key-1234";

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

fn success_body(chunks: &[String], input: u32, output: u32) -> String {
    let mut events = vec![
        (
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":input,"output_tokens":1}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        ("ping", json!({"type":"ping"})),
    ];
    for c in chunks {
        events.push((
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":c}}),
        ));
    }
    events.push((
        "content_block_stop",
        json!({"type":"content_block_stop","index":0}),
    ));
    events.push(("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":output}})));
    events.push(("message_stop", json!({"type":"message_stop"})));
    sse(&events)
}

fn event_stream(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

fn error_body(kind: &str, message: &str) -> Value {
    json!({"type":"error","error":{"type":kind,"message":message}})
}

struct Harness {
    servers: Mutex<Vec<MockServer>>,
    secrets: Arc<InMemorySecrets>,
}

impl Harness {
    async fn new() -> Self {
        let secrets = Arc::new(InMemorySecrets::default());
        secrets
            .set(&SecretRef::new("anthropic"), SecretString::from(KEY))
            .await
            .expect("store key");
        Self {
            servers: Mutex::new(Vec::new()),
            secrets,
        }
    }

    fn provider(&self, server: &MockServer) -> Arc<dyn LlmProvider> {
        let config = AnthropicConfig::new(SecretRef::new("anthropic")).with_base_url(server.uri());
        Arc::new(AnthropicProvider::new(config, self.secrets.clone()).expect("provider"))
    }
}

#[async_trait]
impl LlmContractHarness for Harness {
    async fn provider_for(&self, scenario: &LlmScenario) -> Arc<dyn LlmProvider> {
        let server = MockServer::start().await;
        let messages = Mock::given(method("POST")).and(path("/v1/messages"));
        let models = Mock::given(method("GET")).and(path("/v1/models"));
        match scenario {
            LlmScenario::Success { chunks, usage } => {
                messages
                    .respond_with(event_stream(success_body(
                        chunks,
                        usage.input_tokens,
                        usage.output_tokens,
                    )))
                    .mount(&server)
                    .await;
            }
            LlmScenario::Unauthorized => {
                // Real servers sometimes echo the key back; the adapter must redact it.
                let body = error_body("authentication_error", &format!("invalid x-api-key {KEY}"));
                for m in [messages, models] {
                    m.respond_with(ResponseTemplate::new(401).set_body_json(body.clone()))
                        .mount(&server)
                        .await;
                }
            }
            LlmScenario::RateLimited => {
                messages
                    .respond_with(
                        ResponseTemplate::new(429)
                            .set_body_json(error_body("rate_limit_error", "slow down")),
                    )
                    .mount(&server)
                    .await;
            }
            LlmScenario::ServerError => {
                messages
                    .respond_with(ResponseTemplate::new(529).set_body_json(error_body(
                        "overloaded_error",
                        &format!("Overloaded (key {KEY})"),
                    )))
                    .mount(&server)
                    .await;
            }
            LlmScenario::MalformedBody => {
                messages
                    .respond_with(event_stream("this is not an event stream\n\n".into()))
                    .mount(&server)
                    .await;
            }
            LlmScenario::Models(ids) => {
                let data: Vec<Value> = ids
                    .iter()
                    .map(|id| json!({"type":"model","id":id,"display_name":id.to_uppercase(),"created_at":"2025-01-01T00:00:00Z"}))
                    .collect();
                models
                    .respond_with(
                        ResponseTemplate::new(200)
                            .set_body_json(json!({"data":data,"has_more":false})),
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
async fn anthropic_passes_the_provider_contract() {
    run_llm_provider_contract(&Harness::new().await).await;
}

fn request() -> LlmRequest {
    LlmRequest {
        model: ModelId::new("claude-test"),
        system: Some("Be brief.".into()),
        messages: vec![
            ChatMessage::new(Role::System, "Extra rule."),
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
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
        .and(path("/v1/messages"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(event_stream(success_body(&["ok".into()], 1, 1)))
        .expect(1)
        .mount(&server)
        .await;

    h.provider(&server)
        .complete(request())
        .await
        .expect("reply");

    let received = server.received_requests().await.expect("recording enabled");
    let body: Value = serde_json::from_slice(&received[0].body).expect("json body");
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 256);
    assert_eq!(body["temperature"], 0.25);
    // System text (explicit and from messages) is top-level, never in `messages`.
    assert_eq!(body["system"], "Be brief.\n\nExtra rule.");
    assert_eq!(
        body["messages"],
        json!([
            {"role":"user","content":"hi"},
            {"role":"assistant","content":"hello"},
            {"role":"user","content":"sql please"},
        ])
    );
}

#[tokio::test]
async fn max_tokens_defaults_because_the_api_requires_it_and_optional_fields_are_omitted() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(event_stream(success_body(&["ok".into()], 1, 1)))
        .mount(&server)
        .await;
    let req = LlmRequest {
        model: ModelId::new("m"),
        system: None,
        messages: vec![ChatMessage::user("x")],
        max_tokens: None,
        temperature: None,
    };
    h.provider(&server).complete(req).await.expect("reply");
    let received = server.received_requests().await.expect("recorded");
    let body: Value = serde_json::from_slice(&received[0].body).expect("json");
    assert_eq!(body["max_tokens"], 1024);
    assert!(body.get("system").is_none() && body.get("temperature").is_none());
}

#[tokio::test]
async fn a_missing_key_is_an_authentication_error_and_nothing_is_sent() {
    let server = MockServer::start().await;
    let secrets = Arc::new(InMemorySecrets::default());
    let config = AnthropicConfig::new(SecretRef::new("absent")).with_base_url(server.uri());
    let provider = AnthropicProvider::new(config, secrets).expect("provider");
    let err = provider.complete(request()).await.err();
    assert_eq!(err, Some(application::ports::LlmError::Authentication));
    assert!(
        server
            .received_requests()
            .await
            .expect("recorded")
            .is_empty(),
        "no request without a key"
    );
}

#[tokio::test]
async fn connection_failures_are_network_errors_without_the_key() {
    let h = Harness::new().await;
    // A port that was just free and has nothing listening.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let config = AnthropicConfig::new(SecretRef::new("anthropic"))
        .with_base_url(format!("http://127.0.0.1:{port}"));
    let provider = AnthropicProvider::new(config, h.secrets.clone()).expect("provider");
    let err = provider
        .complete(request())
        .await
        .expect_err("nothing is listening");
    assert!(
        matches!(err, application::ports::LlmError::Network(_)),
        "{err:?}"
    );
    assert!(!format!("{err:?}").contains(KEY));
}

#[tokio::test]
async fn provider_id_is_anthropic() {
    let h = Harness::new().await;
    let server = MockServer::start().await;
    assert_eq!(h.provider(&server).id().as_str(), "anthropic");
}
