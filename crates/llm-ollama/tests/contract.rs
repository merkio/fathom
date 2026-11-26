//! Runs the shared `LlmProvider` contract against a mock Ollama server, and checks the exact
//! request the adapter sends. Fixtures follow the documented `/api/chat` and `/api/tags` formats.
#![allow(clippy::expect_used)] // test helpers

use std::sync::{Arc, Mutex};

use application::ports::{LlmError, LlmProvider, LlmRequest};
use async_trait::async_trait;
use domain::{ChatMessage, ModelId, Role};
use llm_ollama::{OllamaConfig, OllamaProvider};
use serde_json::{Value, json};
use test_support::llm_contract::{LlmContractHarness, LlmScenario, run_llm_provider_contract};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn success_body(chunks: &[String], prompt: u32, eval: u32) -> String {
    let mut out = String::new();
    for c in chunks {
        out.push_str(&format!("{}\n", json!({"model":"m","created_at":"t","message":{"role":"assistant","content":c},"done":false})));
    }
    out.push_str(&format!(
        "{}\n",
        json!({"model":"m","created_at":"t","message":{"role":"assistant","content":""},"done_reason":"stop","done":true,"total_duration":1,"prompt_eval_count":prompt,"eval_count":eval})
    ));
    out
}

fn ndjson(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/x-ndjson")
        .set_body_string(body)
}

struct Harness {
    servers: Mutex<Vec<MockServer>>,
}

impl Harness {
    fn provider(server: &MockServer) -> Arc<dyn LlmProvider> {
        Arc::new(OllamaProvider::new(OllamaConfig::with_base_url(server.uri())).expect("provider"))
    }
}

#[async_trait]
impl LlmContractHarness for Harness {
    async fn provider_for(&self, scenario: &LlmScenario) -> Arc<dyn LlmProvider> {
        let server = MockServer::start().await;
        let chat = Mock::given(method("POST")).and(path("/api/chat"));
        let tags = Mock::given(method("GET")).and(path("/api/tags"));
        match scenario {
            LlmScenario::Success { chunks, usage } => {
                chat.respond_with(ndjson(success_body(
                    chunks,
                    usage.input_tokens,
                    usage.output_tokens,
                )))
                .mount(&server)
                .await;
            }
            // Ollama has no auth of its own; a reverse proxy in front of it might.
            LlmScenario::Unauthorized => {
                for m in [chat, tags] {
                    m.respond_with(
                        ResponseTemplate::new(401).set_body_json(json!({"error":"unauthorized"})),
                    )
                    .mount(&server)
                    .await;
                }
            }
            LlmScenario::RateLimited => {
                chat.respond_with(
                    ResponseTemplate::new(429).set_body_json(json!({"error":"too many requests"})),
                )
                .mount(&server)
                .await;
            }
            LlmScenario::ServerError => {
                chat.respond_with(
                    ResponseTemplate::new(500)
                        .set_body_json(json!({"error":"llama runner process has terminated"})),
                )
                .mount(&server)
                .await;
            }
            LlmScenario::MalformedBody => {
                chat.respond_with(ndjson("this is not json\n".into()))
                    .mount(&server)
                    .await;
            }
            LlmScenario::Models(ids) => {
                let models: Vec<Value> = ids
                    .iter()
                    .map(|id| json!({"name":id,"model":id,"size":1,"digest":"d"}))
                    .collect();
                tags.respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"models":models})),
                )
                .mount(&server)
                .await;
            }
        }
        let provider = Harness::provider(&server);
        self.servers.lock().expect("lock").push(server);
        provider
    }
}

#[tokio::test]
async fn ollama_passes_the_provider_contract() {
    run_llm_provider_contract(&Harness {
        servers: Mutex::new(Vec::new()),
    })
    .await;
}

fn request() -> LlmRequest {
    LlmRequest {
        model: ModelId::new("llama3"),
        system: Some("Be brief.".into()),
        messages: vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant("hello"),
            ChatMessage::new(Role::User, "sql please"),
        ],
        max_tokens: Some(256),
        temperature: Some(0.25),
    }
}

#[tokio::test]
async fn sends_the_documented_request_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson(success_body(&["ok".into()], 1, 1)))
        .expect(1)
        .mount(&server)
        .await;
    Harness::provider(&server)
        .complete(request())
        .await
        .expect("reply");

    let received = server.received_requests().await.expect("recorded");
    let body: Value = serde_json::from_slice(&received[0].body).expect("json");
    assert_eq!(body["model"], "llama3");
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["options"],
        json!({"temperature": 0.25, "num_predict": 256})
    );
    assert_eq!(
        body["messages"],
        json!([
            {"role":"system","content":"Be brief."},
            {"role":"user","content":"hi"},
            {"role":"assistant","content":"hello"},
            {"role":"user","content":"sql please"},
        ])
    );
    assert!(
        received[0].headers.get("authorization").is_none(),
        "Ollama needs no credentials"
    );
}

#[tokio::test]
async fn options_are_omitted_when_not_set() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ndjson(success_body(&["ok".into()], 1, 1)))
        .mount(&server)
        .await;
    let req = LlmRequest {
        model: ModelId::new("m"),
        system: None,
        messages: vec![ChatMessage::user("x")],
        max_tokens: None,
        temperature: None,
    };
    Harness::provider(&server)
        .complete(req)
        .await
        .expect("reply");
    let received = server.received_requests().await.expect("recorded");
    let body: Value = serde_json::from_slice(&received[0].body).expect("json");
    assert!(body.get("options").is_none());
}

#[tokio::test]
async fn an_unknown_model_surfaces_ollamas_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"error":"model 'nope' not found"})),
        )
        .mount(&server)
        .await;
    let err = Harness::provider(&server)
        .complete(request())
        .await
        .expect_err("404");
    assert_eq!(
        err,
        LlmError::Provider("HTTP 404: model 'nope' not found".into())
    );
}

#[tokio::test]
async fn an_error_line_mid_stream_is_reported() {
    let server = MockServer::start().await;
    let body = format!(
        "{}\n{}\n",
        json!({"message":{"role":"assistant","content":"par"},"done":false}),
        json!({"error":"out of memory"})
    );
    Mock::given(method("POST"))
        .respond_with(ndjson(body))
        .mount(&server)
        .await;
    let err = Harness::provider(&server)
        .complete(request())
        .await
        .expect_err("error line");
    assert_eq!(err, LlmError::Provider("out of memory".into()));
}

#[tokio::test]
async fn a_server_that_is_not_running_is_a_network_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port();
    let provider = OllamaProvider::new(OllamaConfig::with_base_url(format!(
        "http://127.0.0.1:{port}"
    )))
    .expect("provider");
    let err = provider
        .complete(request())
        .await
        .expect_err("nothing listens");
    assert!(matches!(err, LlmError::Network(_)), "{err:?}");
}

#[tokio::test]
async fn provider_id_is_ollama_and_the_default_points_at_localhost() {
    assert_eq!(OllamaConfig::default().base_url, "http://localhost:11434");
    let server = MockServer::start().await;
    assert_eq!(Harness::provider(&server).id().as_str(), "ollama");
}
