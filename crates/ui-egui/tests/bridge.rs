//! End-to-end bridge test: real tokio runtime thread, fake driver and in-memory adapters.
#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use application::ports::SystemClock;
use application::registry::{DriverRegistry, LlmRegistry};
use application::usecases::{ExecuteOptions, Ports, UseCases};
use domain::{
    Catalog, ConnectionProfile, DriverKind, QueryResult, ResultColumn, Schema, SchemaTree, Table,
    TableKind, Value,
};
use test_support::{
    FakeDriver, InMemoryConnections, InMemoryHistory, InMemorySavedQueries, InMemorySecrets,
    InMemorySettings,
};
use ui_egui::{Backend, Command, Event, SessionId, TabId};

fn setup() -> (Backend, Receiver<Event>, FakeDriver) {
    let schema = SchemaTree {
        catalogs: vec![Catalog {
            name: "db".into(),
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![Table::new("users", TableKind::Table)],
            }],
        }],
    };
    let result = QueryResult {
        columns: vec![ResultColumn {
            name: "n".into(),
            data_type: domain::DataType::new("int"),
        }],
        rows: vec![vec![Value::Int(1)]],
        ..QueryResult::empty()
    };
    let driver = FakeDriver::new("fake")
        .with_schema(schema)
        .with_result(result);
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(driver.clone()));
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(LlmRegistry::new()),
            settings: Arc::new(InMemorySettings::default()),
            saved_queries: Arc::new(InMemorySavedQueries::default()),
        },
        ExecuteOptions::default(),
    );
    let (backend, events) = Backend::spawn(Arc::new(use_cases), || {});
    (backend, events, driver)
}

fn next(events: &Receiver<Event>) -> Event {
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("event within 5s")
}

#[test]
fn connect_browse_query_history_disconnect() {
    let (backend, events, driver) = setup();
    let profile = ConnectionProfile::new("fake", DriverKind::new("fake"), "localhost");
    let id = profile.id;

    backend.send(Command::SaveConnection {
        profile,
        password: None,
    });
    assert!(matches!(next(&events), Event::Saved(Ok(_))));
    backend.send(Command::LoadConnections);
    assert!(matches!(next(&events), Event::Connections(Ok(list)) if list.len() == 1));

    backend.send(Command::Connect(id));
    let Event::Connected {
        result: Ok(session),
        ..
    } = next(&events)
    else {
        panic!("expected a session")
    };

    backend.send(Command::LoadSchema(session));
    assert!(
        matches!(next(&events), Event::Schema { result: Ok(t), .. } if t.tables().count() == 1)
    );

    backend.send(Command::Execute {
        tab: TabId(1),
        session,
        connection: id,
        text: "select 1".into(),
    });
    assert!(
        matches!(next(&events), Event::QueryFinished { result: Ok(r), .. } if r.rows.len() == 1)
    );
    assert_eq!(driver.executed(), vec!["select 1".to_string()]);

    backend.send(Command::LoadHistory(id));
    assert!(matches!(next(&events), Event::History { result: Ok(h), .. }
        if h.len() == 1 && h[0].text == "select 1" && h[0].succeeded));

    backend.send(Command::Disconnect(session));
    assert!(matches!(next(&events), Event::Disconnected(s) if s == session));

    // After disconnecting, the session is gone and queries fail cleanly.
    backend.send(Command::Execute {
        tab: TabId(1),
        session,
        connection: id,
        text: "select 1".into(),
    });
    assert!(matches!(
        next(&events),
        Event::QueryFinished { result: Err(_), .. }
    ));
}

#[test]
fn connecting_to_an_unknown_connection_reports_an_error() {
    let (backend, events, _) = setup();
    backend.send(Command::Connect(domain::ConnectionId::new()));
    assert!(matches!(
        next(&events),
        Event::Connected { result: Err(_), .. }
    ));
    // Commands for sessions that never existed are answered, not ignored or panicking.
    backend.send(Command::LoadSchema(SessionId(99)));
    assert!(matches!(
        next(&events),
        Event::Schema { result: Err(_), .. }
    ));
}

#[test]
fn notify_is_called_for_every_event() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(FakeDriver::new("fake")));
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(LlmRegistry::new()),
            settings: Arc::new(InMemorySettings::default()),
            saved_queries: Arc::new(InMemorySavedQueries::default()),
        },
        ExecuteOptions::default(),
    );
    let c = counter.clone();
    let (backend, events) = Backend::spawn(Arc::new(use_cases), move || {
        c.fetch_add(1, Ordering::SeqCst);
    });
    backend.send(Command::LoadConnections);
    assert!(matches!(next(&events), Event::Connections(Ok(_))));
    // `notify` runs right after the event is sent, so give it a moment instead of racing it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

// ---- AI and read-only, end to end through the runtime -----------------------------------

use application::ports::{LlmChunk, LlmProvider, LlmRequest, LlmResponse, ModelInfo};
use application::usecases::AiSettings;
use domain::{ChatMessage, Dialect};
use futures::stream::{self, BoxStream};
use secrecy::SecretString;
use test_support::FakeLlm;
use ui_egui::{AiAsk, AiId, AiTask};

/// A provider whose reply never finishes: one chunk, then silence. Used to test stopping.
struct HangingLlm;

#[async_trait::async_trait]
impl LlmProvider for HangingLlm {
    fn id(&self) -> domain::ProviderId {
        domain::ProviderId::new("hang")
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, application::ports::LlmError> {
        Ok(Vec::new())
    }
    async fn complete(&self, _: LlmRequest) -> Result<LlmResponse, application::ports::LlmError> {
        std::future::pending().await
    }
    fn stream(
        &self,
        _: LlmRequest,
    ) -> BoxStream<'static, Result<LlmChunk, application::ports::LlmError>> {
        use futures::StreamExt;
        stream::iter([Ok(LlmChunk::Text("```sql\nSELECT 1".into()))])
            .chain(stream::pending())
            .boxed()
    }
}

fn ai_env(llm: &FakeLlm) -> (Backend, Receiver<Event>, FakeDriver, Arc<InMemorySecrets>) {
    let driver = FakeDriver::new("fake");
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(driver.clone()));
    let mut providers = LlmRegistry::new();
    providers.register_with_key(Arc::new(llm.clone()), domain::SecretRef::new("llm/fake"));
    providers.register(Arc::new(HangingLlm));
    let secrets = Arc::new(InMemorySecrets::default());
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: secrets.clone(),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(providers),
            settings: Arc::new(InMemorySettings::default()),
            saved_queries: Arc::new(InMemorySavedQueries::default()),
        },
        ExecuteOptions::default(),
    );
    let (backend, events) = Backend::spawn(Arc::new(use_cases), || {});
    (backend, events, driver, secrets)
}

fn ask(id: u64, provider: &str, task: AiTask, rows: Option<QueryResult>) -> AiAsk {
    AiAsk {
        id: AiId(id),
        provider: domain::ProviderId::new(provider),
        model: domain::ModelId::new("m"),
        dialect: Dialect {
            name: "postgres",
            ..Dialect::ANSI
        },
        schema: SchemaTree::default(),
        task,
        sample_rows: rows,
    }
}

fn generate(question: &str) -> AiTask {
    AiTask::Generate {
        question: question.into(),
        current_sql: None,
    }
}

#[test]
fn ai_replies_stream_then_finish_with_parsed_sql_and_a_safety_classification() {
    let llm = FakeLlm::new(
        "fake",
        &["```sql\nDROP ", "TABLE users\n```", "\nRemoves it."],
    );
    let (backend, events, driver, _) = ai_env(&llm);

    backend.send(Command::AiAsk(Box::new(ask(
        1,
        "fake",
        generate("remove users"),
        None,
    ))));
    let mut streamed = String::new();
    let reply = loop {
        match next(&events) {
            Event::AiText { id, text } => {
                assert_eq!(id, AiId(1));
                streamed.push_str(&text);
            }
            Event::AiFinished { id, result } => {
                assert_eq!(id, AiId(1));
                break result.expect("reply");
            }
            other => panic!("unexpected {other:?}"),
        }
    };
    assert_eq!(streamed, "```sql\nDROP TABLE users\n```\nRemoves it.");
    assert_eq!(reply.sql.as_deref(), Some("DROP TABLE users"));
    let overall = reply.safety.and_then(|s| s.overall());
    assert_eq!(overall.map(|k| k.label()), Some("ddl"));
    // The assistant has no way to run it: nothing reached the database.
    assert!(driver.executed().is_empty());
}

#[test]
fn what_the_provider_receives_contains_schema_and_question_and_rows_only_on_opt_in() {
    let llm = FakeLlm::new("fake", &["ok"]);
    let (backend, events, _, _) = ai_env(&llm);
    let rows = QueryResult {
        columns: vec![ResultColumn {
            name: "name".into(),
            data_type: domain::DataType::new("text"),
        }],
        rows: vec![vec![Value::Text("TOP-SECRET".into())]],
        ..QueryResult::empty()
    };

    backend.send(Command::AiAsk(Box::new(ask(
        1,
        "fake",
        generate("list"),
        None,
    ))));
    while !matches!(next(&events), Event::AiFinished { .. }) {}
    assert!(!format!("{:?}", llm.requests()).contains("TOP-SECRET"));

    backend.send(Command::AiAsk(Box::new(ask(
        2,
        "fake",
        generate("list"),
        Some(rows),
    ))));
    while !matches!(next(&events), Event::AiFinished { .. }) {}
    assert!(
        format!("{:?}", llm.requests()).contains("TOP-SECRET"),
        "opt-in shares the rows"
    );
}

#[test]
fn preview_returns_the_prompt_without_calling_the_provider() {
    let llm = FakeLlm::new("fake", &["ok"]);
    let (backend, events, _, _) = ai_env(&llm);
    backend.send(Command::AiPreview(Box::new(ask(
        1,
        "fake",
        generate("list customers"),
        None,
    ))));
    let Event::AiPreview(preview) = next(&events) else {
        panic!("expected a preview")
    };
    assert!(preview.messages[0].content.contains("list customers"));
    assert!(!preview.includes_sample_rows);
    assert!(llm.requests().is_empty(), "previewing sends nothing");
}

#[test]
fn stopping_a_hung_reply_delivers_the_partial_text_with_a_note() {
    let llm = FakeLlm::new("fake", &[]);
    let (backend, events, _, _) = ai_env(&llm);
    backend.send(Command::AiAsk(Box::new(ask(
        7,
        "hang",
        generate("x"),
        None,
    ))));
    assert!(matches!(next(&events), Event::AiText { id: AiId(7), .. }));

    backend.send(Command::AiStop(AiId(7)));
    let Event::AiFinished { id, result } = next(&events) else {
        panic!("expected the stop to finish")
    };
    assert_eq!(id, AiId(7));
    let reply = result.expect("partial reply");
    assert_eq!(
        reply.sql.as_deref(),
        Some("SELECT 1"),
        "what arrived is kept"
    );
    assert!(reply.warning.is_some_and(|w| w.contains("Stopped by you")));
}

#[test]
fn unknown_providers_and_failing_providers_end_with_an_error() {
    let llm = FakeLlm::new("fake", &[]).failing(application::ports::LlmError::RateLimited);
    let (backend, events, _, _) = ai_env(&llm);
    backend.send(Command::AiAsk(Box::new(ask(
        1,
        "nope",
        generate("x"),
        None,
    ))));
    assert!(matches!(
        next(&events),
        Event::AiFinished { result: Err(_), .. }
    ));
    backend.send(Command::AiAsk(Box::new(ask(
        2,
        "fake",
        generate("x"),
        None,
    ))));
    let Event::AiFinished {
        result: Err(message),
        ..
    } = next(&events)
    else {
        panic!("expected an error")
    };
    assert!(message.contains("rate limiting"), "{message}");
}

#[test]
fn chat_history_reaches_the_provider() {
    let llm = FakeLlm::new("fake", &["ok"]);
    let (backend, events, _, _) = ai_env(&llm);
    let task = AiTask::Chat {
        history: vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")],
        question: "and now?".into(),
    };
    backend.send(Command::AiAsk(Box::new(ask(1, "fake", task, None))));
    while !matches!(next(&events), Event::AiFinished { .. }) {}
    let sent = llm.requests().pop().expect("request");
    assert_eq!(sent.messages.len(), 3);
    assert_eq!(sent.messages[2].content, "and now?");
}

#[test]
fn settings_models_and_keys_round_trip_without_exposing_the_key() {
    let llm = FakeLlm::new("fake", &[]).with_models(&["a", "b"]);
    let (backend, events, _, secrets) = ai_env(&llm);

    backend.send(Command::LoadAiSettings);
    assert!(matches!(next(&events), Event::AiSettings(Ok(s)) if s == AiSettings::default()));
    let chosen = AiSettings {
        provider: Some(domain::ProviderId::new("fake")),
        model: Some(domain::ModelId::new("a")),
    };
    backend.send(Command::SaveAiSettings(chosen.clone()));
    backend.send(Command::LoadAiSettings);
    assert!(matches!(next(&events), Event::AiSettings(Ok(s)) if s == chosen));

    backend.send(Command::ListModels(domain::ProviderId::new("fake")));
    let Event::Models {
        result: Ok(models), ..
    } = next(&events)
    else {
        panic!("expected models")
    };
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );

    backend.send(Command::LoadApiKeyStatus(domain::ProviderId::new("fake")));
    assert!(matches!(
        next(&events),
        Event::ApiKeyStatus {
            set: Some(false),
            ..
        }
    ));
    backend.send(Command::SaveApiKey {
        provider: domain::ProviderId::new("fake"),
        key: SecretString::from("sk-test"),
    });
    assert!(matches!(next(&events), Event::Notice(Ok(_))));
    let Event::ApiKeyStatus { set, provider } = next(&events) else {
        panic!("expected status")
    };
    assert_eq!((set, provider.as_str()), (Some(true), "fake"));
    let stored = futures::executor::block_on(application::ports::SecretStore::get(
        secrets.as_ref(),
        &domain::SecretRef::new("llm/fake"),
    ))
    .expect("get")
    .expect("stored");
    assert_eq!(secrecy::ExposeSecret::expose_secret(&stored), "sk-test");

    backend.send(Command::ClearApiKey(domain::ProviderId::new("fake")));
    assert!(matches!(next(&events), Event::Notice(Ok(_))));
    assert!(matches!(
        next(&events),
        Event::ApiKeyStatus {
            set: Some(false),
            ..
        }
    ));
}

#[test]
fn read_only_connections_block_writes_before_the_database_and_allow_reads() {
    let llm = FakeLlm::new("fake", &[]);
    let (backend, events, driver, _) = ai_env(&llm);
    let mut profile = ConnectionProfile::new("ro", DriverKind::new("fake"), "localhost");
    profile.set_read_only(true);
    let id = profile.id;
    backend.send(Command::SaveConnection {
        profile,
        password: None,
    });
    assert!(matches!(next(&events), Event::Saved(Ok(_))));
    backend.send(Command::Connect(id));
    let Event::Connected {
        result: Ok(session),
        ..
    } = next(&events)
    else {
        panic!("connect")
    };

    for sql in [
        "DELETE FROM users",
        "DROP TABLE users",
        "SELECT 1; UPDATE t SET a = 1",
    ] {
        backend.send(Command::Execute {
            tab: TabId(1),
            session,
            connection: id,
            text: sql.into(),
        });
        let Event::QueryFinished {
            result: Err(message),
            ..
        } = next(&events)
        else {
            panic!("{sql} should be blocked")
        };
        assert!(message.contains("Read-only mode"), "{message}");
    }
    assert!(driver.executed().is_empty(), "nothing reached the database");

    backend.send(Command::Execute {
        tab: TabId(1),
        session,
        connection: id,
        text: "SELECT 1".into(),
    });
    assert!(matches!(
        next(&events),
        Event::QueryFinished { result: Ok(_), .. }
    ));
    assert_eq!(driver.executed(), vec!["SELECT 1".to_string()]);
}

// ---- LLM inline suggestions, end to end --------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};

use application::usecases::InlineSettings;

/// A provider that answers with one chunk and then hangs, counting its live streams so tests
/// can prove that cancelling drops the stream (which is what closes the connection).
struct CountingHang {
    live: Arc<AtomicUsize>,
}

struct LiveGuard(Arc<AtomicUsize>);

impl Drop for LiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl LlmProvider for CountingHang {
    fn id(&self) -> domain::ProviderId {
        domain::ProviderId::new("hang")
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, application::ports::LlmError> {
        Ok(Vec::new())
    }
    async fn complete(&self, _: LlmRequest) -> Result<LlmResponse, application::ports::LlmError> {
        std::future::pending().await
    }
    fn stream(
        &self,
        _: LlmRequest,
    ) -> BoxStream<'static, Result<LlmChunk, application::ports::LlmError>> {
        use futures::StreamExt;
        self.live.fetch_add(1, Ordering::SeqCst);
        let guard = LiveGuard(self.live.clone());
        // The guard lives in the stream's state and drops with it.
        stream::unfold((guard, false), |(guard, sent)| async move {
            if sent {
                std::future::pending::<()>().await;
                None
            } else {
                Some((Ok(LlmChunk::Text("cust".into())), (guard, true)))
            }
        })
        .boxed()
    }
}

struct InlineEnv {
    backend: Backend,
    events: Receiver<Event>,
    llm: FakeLlm,
    live: Arc<AtomicUsize>,
}

fn inline_env(chunks: &[&str]) -> InlineEnv {
    let mut customers = domain::Table::new("customers", domain::TableKind::Table);
    customers.columns.push(domain::Column {
        name: "id".into(),
        data_type: domain::DataType::new("integer"),
        nullable: false,
        default: None,
    });
    let schema = SchemaTree {
        catalogs: vec![domain::Catalog {
            name: "d".into(),
            schemas: vec![domain::Schema {
                name: "public".into(),
                tables: vec![customers],
            }],
        }],
    };
    let driver = FakeDriver::new("fake").with_schema(schema);
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(driver));
    let llm = FakeLlm::new("fake", chunks);
    let live = Arc::new(AtomicUsize::new(0));
    let mut providers = LlmRegistry::new();
    providers.register(Arc::new(llm.clone()));
    providers.register(Arc::new(CountingHang { live: live.clone() }));
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(providers),
            settings: Arc::new(InMemorySettings::default()),
            saved_queries: Arc::new(InMemorySavedQueries::default()),
        },
        ExecuteOptions::default(),
    );
    let (backend, events) = Backend::spawn(Arc::new(use_cases), || {});
    InlineEnv {
        backend,
        events,
        llm,
        live,
    }
}

impl InlineEnv {
    fn enable(&self, provider: &str) {
        self.backend
            .send(Command::SaveInlineSettings(InlineSettings {
                enabled: true,
                provider: Some(domain::ProviderId::new(provider)),
                model: Some(domain::ModelId::new("m")),
            }));
    }

    /// Saves a connection (optionally with an inline override), connects and loads its schema.
    fn connect(&self, inline_override: Option<bool>) -> (SessionId, domain::ConnectionId) {
        let mut profile = ConnectionProfile::new("c", DriverKind::new("fake"), "localhost");
        profile.set_inline_ai(inline_override);
        let id = profile.id;
        self.backend.send(Command::SaveConnection {
            profile,
            password: None,
        });
        assert!(matches!(next(&self.events), Event::Saved(Ok(_))));
        self.backend.send(Command::Connect(id));
        let Event::Connected {
            result: Ok(session),
            ..
        } = next(&self.events)
        else {
            panic!("connect")
        };
        self.backend.send(Command::LoadSchema(session));
        assert!(matches!(
            next(&self.events),
            Event::Schema { result: Ok(_), .. }
        ));
        (session, id)
    }

    fn ask(&self, session: SessionId, id: u64) {
        self.backend.send(Command::InlineSuggest {
            id: AiId(id),
            session,
            text: "SELECT * FROM ".into(),
            cursor: 14,
        });
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn inline_suggestions_are_off_by_default_and_nothing_reaches_the_provider() {
    let e = inline_env(&["customers"]);
    let (session, _) = e.connect(None);
    e.ask(session, 1);
    assert!(matches!(
        next(&e.events),
        Event::InlineDone {
            id: AiId(1),
            error: None
        }
    ));
    assert!(e.llm.requests().is_empty(), "nothing was sent");
}

#[test]
fn enabled_suggestions_stream_in_labelled_and_small() {
    let e = inline_env(&["cust", "omers"]);
    e.enable("fake");
    let (session, _) = e.connect(None);
    e.ask(session, 7);

    let mut texts = Vec::new();
    loop {
        match next(&e.events) {
            Event::InlineText { id, completion } => {
                assert_eq!(id, AiId(7));
                assert_eq!(completion.safety.map(|k| k.label()), Some("read"));
                texts.push(completion.suggestion.text);
            }
            Event::InlineDone { id, error } => {
                assert_eq!((id, error), (AiId(7), None));
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(texts, ["cust", "customers"]);
    let sent = e.llm.requests().pop().expect("one request");
    assert_eq!((sent.max_tokens, sent.temperature), (Some(64), Some(0.0)));
    assert_eq!(sent.messages[0].content, "SELECT * FROM <CURSOR>");
    assert!(
        sent.system.expect("system").contains("public.customers("),
        "the cached schema is used"
    );
}

#[test]
fn a_connection_that_opted_out_sends_nothing_even_when_the_global_switch_is_on() {
    let e = inline_env(&["customers"]);
    e.enable("fake");
    let (session, _) = e.connect(Some(false));
    e.ask(session, 1);
    assert!(matches!(
        next(&e.events),
        Event::InlineDone { error: None, .. }
    ));
    assert!(e.llm.requests().is_empty());
}

#[test]
fn a_connection_can_opt_in_while_the_global_switch_is_off() {
    let e = inline_env(&["customers"]);
    e.backend.send(Command::SaveInlineSettings(InlineSettings {
        enabled: false,
        provider: Some(domain::ProviderId::new("fake")),
        model: Some(domain::ModelId::new("m")),
    }));
    let (session, _) = e.connect(Some(true));
    e.ask(session, 1);
    assert!(matches!(next(&e.events), Event::InlineText { .. }));
    assert_eq!(e.llm.requests().len(), 1);
}

#[test]
fn cancelling_a_stale_request_drops_the_provider_stream() {
    let e = inline_env(&[]);
    e.enable("hang");
    let (session, _) = e.connect(None);
    e.ask(session, 1);
    assert!(matches!(
        next(&e.events),
        Event::InlineText { id: AiId(1), .. }
    ));
    assert_eq!(e.live.load(Ordering::SeqCst), 1, "the request is in flight");

    e.backend.send(Command::InlineCancel(AiId(1)));
    assert!(matches!(
        next(&e.events),
        Event::InlineDone {
            id: AiId(1),
            error: None
        }
    ));
    wait_for("the stream to be dropped", || {
        e.live.load(Ordering::SeqCst) == 0
    });
}

#[test]
fn a_new_request_after_a_cancel_works() {
    let e = inline_env(&[]);
    e.enable("hang");
    let (session, _) = e.connect(None);
    for id in 1..=3u64 {
        e.ask(session, id);
        assert!(matches!(next(&e.events), Event::InlineText { .. }));
        e.backend.send(Command::InlineCancel(AiId(id)));
        assert!(matches!(next(&e.events), Event::InlineDone { .. }));
    }
    wait_for("all streams to be dropped", || {
        e.live.load(Ordering::SeqCst) == 0
    });
}

#[test]
fn unknown_sessions_and_missing_schemas_end_quietly_without_contacting_the_provider() {
    let e = inline_env(&["x"]);
    e.enable("fake");
    e.ask(SessionId(99), 1);
    assert!(matches!(
        next(&e.events),
        Event::InlineDone { error: None, .. }
    ));

    // Connected but the schema was never loaded: nothing useful to ask.
    let profile = ConnectionProfile::new("c", DriverKind::new("fake"), "localhost");
    let id = profile.id;
    e.backend.send(Command::SaveConnection {
        profile,
        password: None,
    });
    assert!(matches!(next(&e.events), Event::Saved(Ok(_))));
    e.backend.send(Command::Connect(id));
    let Event::Connected {
        result: Ok(session),
        ..
    } = next(&e.events)
    else {
        panic!("connect")
    };
    e.ask(session, 2);
    assert!(matches!(
        next(&e.events),
        Event::InlineDone { error: None, .. }
    ));
    assert!(e.llm.requests().is_empty());
}

#[test]
fn provider_failures_are_reported_in_the_done_event() {
    let mut e = inline_env(&[]);
    // Rebuild with a failing provider.
    let failing = FakeLlm::new("fake", &[]).failing(application::ports::LlmError::RateLimited);
    let mut providers = LlmRegistry::new();
    providers.register(Arc::new(failing));
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(
        FakeDriver::new("fake").with_schema(SchemaTree::default()),
    ));
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(providers),
            settings: Arc::new(InMemorySettings::default()),
            saved_queries: Arc::new(InMemorySavedQueries::default()),
        },
        ExecuteOptions::default(),
    );
    let (backend, events) = Backend::spawn(Arc::new(use_cases), || {});
    e.backend = backend;
    e.events = events;
    e.enable("fake");
    let (session, _) = e.connect(None);
    e.ask(session, 1);
    let Event::InlineDone {
        error: Some(message),
        ..
    } = next(&e.events)
    else {
        panic!("expected an error")
    };
    assert!(message.contains("rate limiting"), "{message}");
}

#[test]
fn inline_settings_round_trip_through_the_backend() {
    let e = inline_env(&[]);
    e.backend.send(Command::LoadInlineSettings);
    assert!(
        matches!(next(&e.events), Event::InlineSettings(Ok(s)) if s == InlineSettings::default())
    );
    e.enable("fake");
    e.backend.send(Command::LoadInlineSettings);
    let Event::InlineSettings(Ok(s)) = next(&e.events) else {
        panic!("settings")
    };
    assert!(s.enabled && s.provider.as_ref().map(|p| p.as_str()) == Some("fake"));
}

#[test]
fn saved_queries_round_trip_through_the_backend_and_names_stay_unique() {
    use application::usecases::SaveDraft;
    let e = inline_env(&[]);
    let draft = |name: &str, text: &str| SaveDraft {
        id: None,
        name: name.into(),
        text: text.into(),
        connection: None,
    };
    e.backend.send(Command::SaveQuery {
        tab: TabId(1),
        draft: draft("customers", "select * from customers"),
    });
    let Event::QuerySaved {
        result: Ok(first), ..
    } = next(&e.events)
    else {
        panic!("expected a saved query")
    };
    e.backend.send(Command::SaveQuery {
        tab: TabId(1),
        draft: draft("CUSTOMERS", "select 2"),
    });
    let Event::QuerySaved {
        result: Err(message),
        ..
    } = next(&e.events)
    else {
        panic!("expected a refusal")
    };
    assert!(message.contains("already exists"), "{message}");

    e.backend.send(Command::LoadSavedQueries);
    let Event::SavedQueries(Ok(list)) = next(&e.events) else {
        panic!("list")
    };
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].text, "select * from customers");

    e.backend.send(Command::DeleteSavedQuery(first.id));
    assert!(matches!(next(&e.events), Event::SavedQueries(Ok(list)) if list.is_empty()));
}

#[test]
fn ui_prefs_round_trip_through_the_backend() {
    use application::usecases::{ThemePreference, UiPrefs};
    let e = inline_env(&[]);
    e.backend.send(Command::LoadUiPrefs);
    assert!(matches!(next(&e.events), Event::UiPrefs(Ok(p)) if p == UiPrefs::default()));
    let saved = UiPrefs {
        theme: ThemePreference::Dark,
        explorer: false,
        results: true,
        side_panel: false,
    };
    e.backend.send(Command::SaveUiPrefs(saved));
    e.backend.send(Command::LoadUiPrefs);
    assert!(matches!(next(&e.events), Event::UiPrefs(Ok(p)) if p == saved));
}

#[test]
fn explorer_visibility_round_trips_per_connection_through_the_backend() {
    let e = inline_env(&[]);
    let (a, b) = (domain::ConnectionId::new(), domain::ConnectionId::new());
    e.backend.send(Command::LoadVisibility(a));
    assert!(matches!(
        next(&e.events),
        Event::Visibility { connection, result: Ok(None) } if connection == a
    ));
    e.backend.send(Command::SaveVisibility {
        connection: a,
        visible: vec!["commerce".into(), "analytics".into()],
    });
    e.backend.send(Command::LoadVisibility(a));
    let Event::Visibility {
        result: Ok(Some(names)),
        ..
    } = next(&e.events)
    else {
        panic!("saved choice")
    };
    assert_eq!(names, ["commerce", "analytics"]);
    e.backend.send(Command::LoadVisibility(b));
    assert!(matches!(
        next(&e.events),
        Event::Visibility {
            result: Ok(None),
            ..
        }
    ));
}

// ---- export, friendly errors and logging --------------------------------------------------

use application::export::ExportFormat;
use application::usecases::ExportOptions;
use ui_egui::ExportDone;

/// An export destination that keeps files in memory and, like every real one, never overwrites.
#[derive(Default)]
struct MemoryDestination(std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>);

#[async_trait::async_trait]
impl application::ports::ExportDestination for MemoryDestination {
    fn kind(&self) -> &'static str {
        "memory"
    }
    fn describe(&self) -> String {
        "memory://".into()
    }
    async fn test(&self) -> Result<(), application::ports::ExportError> {
        Ok(())
    }
    async fn put(
        &self,
        path: &str,
        data: Vec<u8>,
    ) -> Result<String, application::ports::ExportError> {
        use application::ports::ExportError;
        if path.split('/').any(|s| s == "..") {
            return Err(ExportError::Invalid("outside".into()));
        }
        let mut files = self.0.lock().expect("lock");
        if files.contains_key(path) {
            return Err(ExportError::AlreadyExists(path.into()));
        }
        files.insert(path.into(), data);
        Ok(format!("memory://{path}"))
    }
}

impl MemoryDestination {
    fn text(&self, path: &str) -> Option<String> {
        self.0
            .lock()
            .expect("lock")
            .get(path)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

struct ExportEnv {
    backend: Backend,
    events: Receiver<Event>,
    driver: FakeDriver,
    files: Arc<MemoryDestination>,
}

fn export_env(row_count: i64, max_rows: u32) -> ExportEnv {
    let result = QueryResult {
        columns: vec![
            ResultColumn {
                name: "id".into(),
                data_type: domain::DataType::new("int"),
            },
            ResultColumn {
                name: "name".into(),
                data_type: domain::DataType::new("text"),
            },
        ],
        rows: (1..=row_count)
            .map(|i| vec![Value::Int(i), Value::Text(format!("n{i}"))])
            .collect(),
        ..QueryResult::empty()
    };
    let driver = FakeDriver::new("fake").with_result(result);
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(driver.clone()));
    let files = Arc::new(MemoryDestination::default());
    let mut destinations = application::registry::DestinationRegistry::new();
    destinations.register("Memory", files.clone());
    let use_cases = UseCases::new(
        Ports {
            connections: Arc::new(InMemoryConnections::default()),
            history: Arc::new(InMemoryHistory::default()),
            secrets: Arc::new(InMemorySecrets::default()),
            clock: Arc::new(SystemClock),
            drivers: Arc::new(drivers),
            providers: Arc::new(LlmRegistry::new()),
            settings: Arc::new(InMemorySettings::default()),
            saved_queries: Arc::new(InMemorySavedQueries::default()),
        },
        ExecuteOptions::default(),
    )
    .with_export(ExportOptions { max_rows })
    .with_destinations(Arc::new(destinations));
    let (backend, events) = Backend::spawn(Arc::new(use_cases), || {});
    ExportEnv {
        backend,
        events,
        driver,
        files,
    }
}

impl ExportEnv {
    fn connect(&self, read_only: bool) -> (SessionId, domain::ConnectionId) {
        let mut profile = ConnectionProfile::new("c", DriverKind::new("fake"), "localhost");
        profile.set_read_only(read_only);
        let id = profile.id;
        self.backend.send(Command::SaveConnection {
            profile,
            password: None,
        });
        assert!(matches!(next(&self.events), Event::Saved(Ok(_))));
        self.backend.send(Command::Connect(id));
        let Event::Connected {
            result: Ok(session),
            ..
        } = next(&self.events)
        else {
            panic!("connect")
        };
        (session, id)
    }

    fn export(
        &self,
        s: (SessionId, domain::ConnectionId),
        sql: &str,
        format: ExportFormat,
        name: &str,
    ) -> Result<ExportDone, String> {
        self.export_to(s, "Memory", "", sql, format, name)
    }

    fn export_to(
        &self,
        s: (SessionId, domain::ConnectionId),
        destination: &str,
        folder: &str,
        sql: &str,
        format: ExportFormat,
        name: &str,
    ) -> Result<ExportDone, String> {
        self.backend.send(Command::Export {
            tab: TabId(1),
            session: s.0,
            connection: s.1,
            source: ui_egui::ExportSource::Query(sql.into()),
            destination: destination.into(),
            folder: folder.into(),
            name: name.into(),
            format,
            table: Some("people".into()),
        });
        let Event::Exported { result, .. } = next(&self.events) else {
            panic!("expected Exported")
        };
        result
    }
}

fn one_delete() -> domain::TableChanges {
    domain::TableChanges {
        schema: None,
        table: "t".into(),
        column_types: vec![],
        changes: vec![domain::RowChange::Delete {
            key: vec![("id".into(), Value::Int(1))],
        }],
    }
}

#[test]
fn applying_changes_is_blocked_on_a_read_only_connection() {
    let e = export_env(1, 100);
    let (session, _) = e.connect(true);
    e.backend.send(Command::ApplyChanges {
        tab: TabId(1),
        session,
        changes: one_delete(),
    });
    let Event::ChangesApplied { result, .. } = next(&e.events) else {
        panic!("expected ChangesApplied")
    };
    let message = result.expect_err("read-only must refuse");
    assert!(message.contains("read-only"), "{message}");
}

#[test]
fn a_database_without_row_editing_reports_it_instead_of_pretending() {
    // The fake driver does not override `apply_changes`, like any adapter that cannot edit rows.
    let e = export_env(1, 100);
    let (session, _) = e.connect(false);
    e.backend.send(Command::ApplyChanges {
        tab: TabId(1),
        session,
        changes: one_delete(),
    });
    let Event::ChangesApplied { result, .. } = next(&e.events) else {
        panic!("expected ChangesApplied")
    };
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn exports_write_a_new_file_with_the_full_result() {
    let e = export_env(3, 100);
    let s = e.connect(false);
    let done = e
        .export_to(
            s,
            "Memory",
            "exports/today",
            "select * from t",
            ExportFormat::Csv,
            "people",
        )
        .expect("export");
    assert_eq!(done.location, "memory://exports/today/people.csv");
    assert_eq!((done.rows, done.truncated), (3, false));
    assert_eq!(
        e.files.text("exports/today/people.csv").expect("file"),
        "id,name\r\n1,n1\r\n2,n2\r\n3,n3\r\n"
    );

    let json = e
        .export(s, "select * from t", ExportFormat::Json, "people")
        .expect("json");
    assert_eq!(json.location, "memory://people.json");
    assert!(
        e.files
            .text("people.json")
            .expect("file")
            .contains("{\"id\": 1, \"name\": \"n1\"}")
    );

    let sql = e
        .export(s, "select * from t", ExportFormat::Sql, "people")
        .expect("sql");
    assert_eq!(sql.location, "memory://people.sql");
    assert!(
        e.files
            .text("people.sql")
            .expect("file")
            .starts_with("INSERT INTO \"people\" (\"id\", \"name\") VALUES (1, 'n1');")
    );
}

#[test]
fn selected_rows_are_exported_as_given_without_touching_the_database() {
    let e = export_env(3, 100);
    let s = e.connect(false);
    let rows = QueryResult {
        columns: vec![ResultColumn {
            name: "id".into(),
            data_type: domain::DataType::new("int"),
        }],
        rows: vec![vec![Value::Int(7)]],
        ..QueryResult::empty()
    };
    e.backend.send(Command::Export {
        tab: TabId(1),
        session: s.0,
        connection: s.1,
        source: ui_egui::ExportSource::Rows(rows),
        destination: "Memory".into(),
        folder: String::new(),
        name: "picked".into(),
        format: ExportFormat::Csv,
        table: None,
    });
    let Event::Exported {
        result: Ok(done), ..
    } = next(&e.events)
    else {
        panic!("expected a finished export")
    };
    assert_eq!(done.rows, 1);
    assert_eq!(e.files.text("picked.csv").expect("file"), "id\r\n7\r\n");
    assert!(e.driver.executed().is_empty(), "no query was run");
}

#[test]
fn the_row_cap_truncates_the_export_and_says_so() {
    let e = export_env(10, 4);
    let s = e.connect(false);
    let done = e
        .export(s, "select * from t", ExportFormat::Csv, "capped")
        .expect("export");
    assert_eq!((done.rows, done.truncated), (4, true));
    assert_eq!(e.files.text("capped.csv").expect("file").lines().count(), 5);
}

#[test]
fn an_existing_file_is_never_overwritten() {
    let e = export_env(3, 100);
    let s = e.connect(false);
    e.export(s, "select * from t", ExportFormat::Csv, "keep")
        .expect("first");
    let before = e.files.text("keep.csv").expect("file");
    let err = e
        .export(s, "select * from t", ExportFormat::Csv, "keep")
        .expect_err("must refuse");
    assert!(
        err.contains("already exists") && err.contains("never overwritten"),
        "{err}"
    );
    assert_eq!(e.files.text("keep.csv").expect("file"), before);
}

#[test]
fn a_blocked_or_empty_export_writes_nothing() {
    let e = export_env(3, 100);
    let ro = e.connect(true);
    let err = e
        .export(ro, "DELETE FROM t RETURNING *", ExportFormat::Csv, "nope")
        .expect_err("blocked");
    assert!(err.contains("Read-only mode"), "{err}");
    assert!(
        err.contains("Hint:") && err.contains("Turn off read-only mode"),
        "friendly text: {err}"
    );
    assert!(e.files.text("nope.csv").is_none(), "no partial file");
    assert!(
        e.driver.executed().is_empty(),
        "nothing reached the database"
    );
}

#[test]
fn exporting_to_an_unknown_destination_closed_session_or_bad_path_fails_cleanly() {
    let e = export_env(1, 10);
    let (_, id) = e.connect(false);
    let err = e
        .export((SessionId(99), id), "select 1", ExportFormat::Csv, "x")
        .expect_err("closed");
    assert!(err.contains("closed"), "{err}");
    let s = e.connect(false);
    let err = e
        .export_to(s, "Nowhere", "", "select 1", ExportFormat::Csv, "x")
        .expect_err("unknown destination");
    assert!(err.contains("not a configured destination"), "{err}");
    assert!(
        e.export(s, "select 1", ExportFormat::Csv, "folder/x")
            .is_err()
    );
    assert!(
        e.export_to(s, "Memory", "../up", "select 1", ExportFormat::Csv, "x")
            .is_err()
    );
}

#[test]
fn destinations_are_listed_and_tested_through_the_backend() {
    let e = export_env(1, 10);
    e.backend.send(Command::LoadDestinations);
    let Event::Destinations { items, problems } = next(&e.events) else {
        panic!("expected Destinations")
    };
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].name.as_str(), items[0].kind.as_str()),
        ("Memory", "memory")
    );
    assert!(problems.is_empty());
    e.backend.send(Command::TestDestination("Memory".into()));
    assert!(matches!(
        next(&e.events),
        Event::DestinationTested { result: Ok(()), .. }
    ));
    e.backend.send(Command::TestDestination("Nowhere".into()));
    assert!(matches!(
        next(&e.events),
        Event::DestinationTested { result: Err(_), .. }
    ));
}

#[test]
fn query_errors_reach_the_ui_with_a_hint() {
    let e = export_env(1, 10);
    let (session, id) = e.connect(true);
    e.backend.send(Command::Execute {
        tab: TabId(1),
        session,
        connection: id,
        text: "DROP TABLE t".into(),
    });
    let Event::QueryFinished {
        result: Err(message),
        ..
    } = next(&e.events)
    else {
        panic!("blocked")
    };
    let lines: Vec<&str> = message.lines().collect();
    assert!(lines[0].starts_with("Read-only mode"), "{message}");
    assert!(lines[1].starts_with("Hint: "), "{message}");
}

mod logging {
    use std::io::Write;
    use std::sync::{Arc, Mutex, OnceLock};

    use super::*;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn captured() -> Arc<Mutex<Vec<u8>>> {
        static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
        BUFFER
            .get_or_init(|| {
                let buffer = Arc::new(Mutex::new(Vec::new()));
                let writer = Capture(buffer.clone());
                let subscriber = tracing_subscriber::fmt()
                    .with_writer(move || writer.clone())
                    .with_ansi(false)
                    .finish();
                let _ = tracing::subscriber::set_global_default(subscriber);
                buffer
            })
            .clone()
    }

    #[test]
    fn logs_describe_what_happened_without_sql_prompts_rows_or_keys() {
        let buffer = captured();
        let llm = FakeLlm::new("fake", &["ok"]);
        let (backend, events, _, _) = ai_env(&llm);

        let profile = ConnectionProfile::new("c", DriverKind::new("fake"), "localhost");
        let id = profile.id;
        backend.send(Command::SaveConnection {
            profile,
            password: Some(SecretString::from("PASSWORD-ABC123")),
        });
        assert!(matches!(next(&events), Event::Saved(Ok(_))));
        backend.send(Command::Connect(id));
        let Event::Connected {
            result: Ok(session),
            ..
        } = next(&events)
        else {
            panic!("connect")
        };
        backend.send(Command::Execute {
            tab: TabId(1),
            session,
            connection: id,
            text: "SELECT 'TOP-SECRET-LITERAL'".into(),
        });
        assert!(matches!(
            next(&events),
            Event::QueryFinished { result: Ok(_), .. }
        ));

        let rows = QueryResult {
            columns: vec![ResultColumn {
                name: "name".into(),
                data_type: domain::DataType::new("text"),
            }],
            rows: vec![vec![Value::Text("ROW-DATA-VALUE".into())]],
            ..QueryResult::empty()
        };
        backend.send(Command::AiAsk(Box::new(ask(
            1,
            "fake",
            generate("PRIVATE-QUESTION-TEXT"),
            Some(rows),
        ))));
        while !matches!(next(&events), Event::AiFinished { .. }) {}

        let log = String::from_utf8(buffer.lock().expect("lock").clone()).expect("utf8");
        assert!(
            log.contains("connected")
                && log.contains("query finished")
                && log.contains("ai request finished"),
            "{log}"
        );
        for secret in [
            "TOP-SECRET-LITERAL",
            "PRIVATE-QUESTION-TEXT",
            "ROW-DATA-VALUE",
            "PASSWORD-ABC123",
        ] {
            assert!(!log.contains(secret), "the log leaked `{secret}`:\n{log}");
        }
    }
}

#[test]
fn settings_writes_apply_in_the_order_they_were_sent() {
    let llm = FakeLlm::new("fake", &[]);
    let (backend, events, _, _) = ai_env(&llm);
    let settings = |model: &str| AiSettings {
        provider: Some(domain::ProviderId::new("fake")),
        model: Some(domain::ModelId::new(model)),
    };
    // Many rapid writes followed by a read: the read must always see the last write. Before
    // writes were ordered this was a race between independent tasks.
    for round in 0..200 {
        let last = format!("model-{round}");
        for m in ["first", "second", &last] {
            backend.send(Command::SaveAiSettings(settings(m)));
        }
        backend.send(Command::LoadAiSettings);
        let Event::AiSettings(Ok(seen)) = next(&events) else {
            panic!("settings")
        };
        assert_eq!(seen, settings(&last), "round {round}");
    }
}

#[test]
fn only_settings_and_key_writes_are_ordered() {
    let ordered = [
        Command::SaveAiSettings(AiSettings::default()),
        Command::SaveInlineSettings(InlineSettings::default()),
        Command::SaveUiPrefs(application::usecases::UiPrefs::default()),
        Command::DeleteSavedQuery(domain::SavedQueryId::new()),
        Command::SaveVisibility {
            connection: domain::ConnectionId::new(),
            visible: vec![],
        },
        Command::ClearApiKey(domain::ProviderId::new("p")),
    ];
    assert!(ordered.iter().all(Command::is_ordered));
    for command in [
        Command::LoadConnections,
        Command::LoadAiSettings,
        Command::Cancel(SessionId(1)),
        Command::AiStop(AiId(1)),
    ] {
        assert!(!command.is_ordered(), "{command:?} must stay concurrent");
    }
}
