#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;

use application::error::UseCaseError;
use application::llm::StatementKind;
use application::ports::{LlmError, SettingsRepository};
use application::registry::LlmRegistry;
use application::sql::{SuggestionKind, SuggestionSource};
use application::usecases::{
    InlineCompletion, InlineRequest, InlineSettings, InlineSettingsStore, SuggestInline,
};
use domain::{
    Catalog, Column, DataType, Dialect, ModelId, ProviderId, Schema, SchemaTree, Table, TableKind,
};
use futures::StreamExt;
use futures::executor::block_on;
use test_support::{FakeLlm, InMemorySettings};

fn schema() -> SchemaTree {
    let mut customers = Table::new("customers", TableKind::Table);
    customers.columns = ["id", "name"]
        .iter()
        .map(|c| Column {
            name: (*c).into(),
            data_type: DataType::new("text"),
            nullable: true,
            default: None,
        })
        .collect();
    let mut audit = Table::new("audit_log", TableKind::Table);
    audit.columns = vec![Column {
        name: "msg".into(),
        data_type: DataType::new("text"),
        nullable: true,
        default: None,
    }];
    SchemaTree {
        catalogs: vec![Catalog {
            name: "d".into(),
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![customers, audit],
            }],
        }],
    }
}

fn enabled() -> InlineSettings {
    InlineSettings {
        enabled: true,
        provider: Some(ProviderId::new("local")),
        model: Some(ModelId::new("tiny")),
    }
}

struct Env {
    inline: SuggestInline,
    llm: FakeLlm,
    settings: Arc<InMemorySettings>,
}

fn env(chunks: &[&str]) -> Env {
    let llm = FakeLlm::new("local", chunks);
    let mut registry = LlmRegistry::new();
    registry.register(Arc::new(llm.clone()));
    let settings = Arc::new(InMemorySettings::default());
    Env {
        inline: SuggestInline::new(Arc::new(registry), settings.clone()),
        llm,
        settings,
    }
}

fn save(env: &Env, s: &InlineSettings) {
    block_on(InlineSettingsStore::new(env.settings.clone()).save(s)).expect("save");
}

fn start<'a>(
    env: &Env,
    dialect: &'a Dialect,
    tree: &'a SchemaTree,
    text: &'a str,
    cursor: usize,
    connection_override: Option<bool>,
) -> Result<Vec<Result<InlineCompletion, LlmError>>, UseCaseError> {
    let req = InlineRequest {
        dialect,
        schema: tree,
        text,
        cursor,
        connection_override,
    };
    let stream = block_on(env.inline.start(&req))?;
    Ok(block_on(stream.collect()))
}

#[test]
fn it_is_off_by_default() {
    let e = env(&["customers"]);
    assert_eq!(
        block_on(e.inline.settings().load()).expect("load"),
        InlineSettings::default()
    );
    assert!(!InlineSettings::default().enabled);
}

#[test]
fn a_disabled_request_never_reaches_the_provider_even_when_everything_else_is_configured() {
    let e = env(&["customers"]);
    let (d, t) = (Dialect::ANSI, schema());
    // Provider and model are set; only the switch is off. The block must come from the switch.
    save(
        &e,
        &InlineSettings {
            enabled: false,
            ..enabled()
        },
    );

    let result = start(&e, &d, &t, "SELECT * FROM ", 14, None);
    let Err(UseCaseError::Blocked(message)) = result else {
        panic!("expected Blocked, got {result:?}")
    };
    assert!(
        message.contains("off"),
        "blocked because it is switched off: {message}"
    );
    assert!(e.llm.requests().is_empty(), "nothing was sent");
}

#[test]
fn turning_it_off_again_stops_all_traffic_immediately() {
    let e = env(&["customers"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(&e, &enabled());
    start(&e, &d, &t, "SELECT * FROM ", 14, None).expect("enabled");
    assert_eq!(e.llm.requests().len(), 1);

    save(
        &e,
        &InlineSettings {
            enabled: false,
            ..enabled()
        },
    );
    assert!(start(&e, &d, &t, "SELECT * FROM ", 14, None).is_err());
    assert_eq!(e.llm.requests().len(), 1, "no new request after disabling");
}

#[test]
fn the_connection_override_wins_in_both_directions() {
    let e = env(&["x"]);
    let (d, t) = (Dialect::ANSI, schema());

    save(&e, &enabled());
    assert!(matches!(
        start(&e, &d, &t, "SELECT ", 7, Some(false)),
        Err(UseCaseError::Blocked(_))
    ));
    assert!(
        e.llm.requests().is_empty(),
        "a connection that opted out sends nothing"
    );

    save(
        &e,
        &InlineSettings {
            enabled: false,
            ..enabled()
        },
    );
    start(&e, &d, &t, "SELECT ", 7, Some(true)).expect("override on");
    assert_eq!(e.llm.requests().len(), 1);
}

#[test]
fn it_needs_a_provider_and_a_model_and_a_registered_provider() {
    let e = env(&["x"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(
        &e,
        &InlineSettings {
            enabled: true,
            provider: None,
            model: None,
        },
    );
    assert!(matches!(
        start(&e, &d, &t, "S", 1, None),
        Err(UseCaseError::Blocked(_))
    ));
    save(
        &e,
        &InlineSettings {
            enabled: true,
            provider: Some(ProviderId::new("local")),
            model: None,
        },
    );
    assert!(matches!(
        start(&e, &d, &t, "S", 1, None),
        Err(UseCaseError::Blocked(_))
    ));
    save(
        &e,
        &InlineSettings {
            enabled: true,
            provider: Some(ProviderId::new("gone")),
            model: Some(ModelId::new("m")),
        },
    );
    assert!(matches!(
        start(&e, &d, &t, "S", 1, None),
        Err(UseCaseError::UnknownProvider(_))
    ));
    assert!(e.llm.requests().is_empty());
}

#[test]
fn the_request_is_small_deterministic_and_uses_the_inline_model() {
    let e = env(&["customers"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(&e, &enabled());
    start(&e, &d, &t, "SELECT * FROM cu WHERE id = 1", 16, None).expect("start");

    let sent = e.llm.requests().pop().expect("request");
    assert_eq!(sent.model, ModelId::new("tiny"));
    assert_eq!(sent.max_tokens, Some(64));
    assert_eq!(sent.temperature, Some(0.0));
    assert_eq!(sent.messages.len(), 1);
    assert_eq!(
        sent.messages[0].content,
        "SELECT * FROM cu<CURSOR> WHERE id = 1"
    );
    let system = sent.system.expect("system");
    assert!(system.contains("public.customers("), "{system}");
    assert!(
        system.contains("<CURSOR>") && system.contains("ONLY the text to insert"),
        "{system}"
    );
}

#[test]
fn only_schema_metadata_and_limited_editor_text_are_sent() {
    let e = env(&["x"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(&e, &enabled());
    let text = format!(
        "{}SELECT * FROM {}",
        "-- old\n".repeat(1000),
        "x".repeat(2000)
    );
    let cursor = text.find("FROM").expect("marker") + 5;
    start(&e, &d, &t, &text, cursor, None).expect("start");
    let sent = e.llm.requests().pop().expect("request");
    let content = &sent.messages[0].content;

    let (before, after) = content.split_once("<CURSOR>").expect("marker");
    assert!(
        before.chars().count() <= 2000,
        "prefix is limited: {}",
        before.chars().count()
    );
    assert!(
        after.chars().count() <= 500,
        "suffix is limited: {}",
        after.chars().count()
    );
    assert!(
        !format!("{sent:?}")
            .to_lowercase()
            .contains("select name from"),
        "no data, only metadata"
    );
}

#[test]
fn the_schema_is_pruned_to_what_the_editor_mentions() {
    let e = env(&["x"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(&e, &enabled());
    // Pretend the schema is large: the budget keeps only tables related to the text.
    let mut big = t.clone();
    for i in 0..300 {
        let mut table = Table::new(format!("noise_table_number_{i}"), TableKind::Table);
        table.columns = (0..10)
            .map(|c| Column {
                name: format!("column_{c}"),
                data_type: DataType::new("text"),
                nullable: true,
                default: None,
            })
            .collect();
        big.catalogs[0].schemas[0].tables.push(table);
    }
    start(&e, &d, &big, "SELECT * FROM customers WHERE ", 30, None).expect("start");
    let system = e
        .llm
        .requests()
        .pop()
        .expect("request")
        .system
        .expect("system");
    assert!(
        system.contains("public.customers("),
        "the referenced table is kept"
    );
    assert!(
        system.contains("more table(s) omitted"),
        "the rest is dropped: {}",
        system.len()
    );
    assert!(system.len() < 6000, "prompt stays small: {}", system.len());
}

#[test]
fn completions_stream_in_growing_and_are_labelled_as_llm_text_at_the_cursor() {
    let e = env(&["cust", "omers", " c"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(&e, &enabled());
    let out = start(&e, &d, &t, "SELECT * FROM ", 14, None).expect("start");
    let texts: Vec<_> = out
        .iter()
        .map(|c| c.as_ref().expect("ok").suggestion.text.clone())
        .collect();
    assert_eq!(texts, ["cust", "customers", "customers c"]);
    let last = &out.last().expect("last").as_ref().expect("ok").suggestion;
    assert_eq!(
        (last.kind, last.source, last.range.clone()),
        (SuggestionKind::Inline, SuggestionSource::Llm, 14..14)
    );
}

#[test]
fn the_safety_guard_labels_the_statement_the_completion_would_make() {
    let (d, t) = (
        Dialect {
            name: "postgres",
            ..Dialect::ANSI
        },
        schema(),
    );
    for (prefix, completion, expected) in [
        ("SELECT * FROM ", "customers", StatementKind::Read),
        ("DELETE FROM ", "customers", StatementKind::Write),
        ("DROP TABLE ", "customers", StatementKind::Ddl),
    ] {
        let e = env(&[completion]);
        save(&e, &enabled());
        let out = start(&e, &d, &t, prefix, prefix.len(), None).expect("start");
        assert_eq!(
            out[0].as_ref().expect("ok").safety,
            Some(expected),
            "{prefix}{completion}"
        );
    }
}

#[test]
fn fences_and_chatter_are_cleaned_and_empty_replies_produce_nothing() {
    let (d, t) = (Dialect::ANSI, schema());
    let e = env(&["```sql\ncustomers\n```\nThat joins..."]);
    save(&e, &enabled());
    let out = start(&e, &d, &t, "SELECT * FROM ", 14, None).expect("start");
    assert_eq!(
        out.last()
            .expect("one")
            .as_ref()
            .expect("ok")
            .suggestion
            .text,
        "customers"
    );

    let e = env(&["   ", "\n"]);
    save(&e, &enabled());
    assert!(
        start(&e, &d, &t, "SELECT ", 7, None)
            .expect("start")
            .is_empty(),
        "blank replies show no ghost text"
    );
}

#[test]
fn provider_errors_are_delivered_as_stream_items() {
    let llm = FakeLlm::new("local", &[]).failing(LlmError::RateLimited);
    let mut registry = LlmRegistry::new();
    registry.register(Arc::new(llm));
    let settings = Arc::new(InMemorySettings::default());
    let inline = SuggestInline::new(Arc::new(registry), settings.clone());
    block_on(InlineSettingsStore::new(settings.clone()).save(&enabled())).expect("save");
    let (d, t) = (Dialect::ANSI, schema());
    let req = InlineRequest {
        dialect: &d,
        schema: &t,
        text: "S",
        cursor: 1,
        connection_override: None,
    };
    let items: Vec<_> = block_on(block_on(inline.start(&req)).expect("start").collect());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].as_ref().err(), Some(&LlmError::RateLimited));
}

#[test]
fn a_cursor_inside_a_multibyte_character_is_clamped() {
    let e = env(&["x"]);
    let (d, t) = (Dialect::ANSI, schema());
    save(&e, &enabled());
    start(&e, &d, &t, "SELECT 'é'", 9, None).expect("no panic");
    start(&e, &d, &t, "é", 1, None).expect("no panic");
    start(&e, &d, &t, "abc", 99, None).expect("no panic");
}

#[test]
fn settings_round_trip_and_the_flag_is_stored_as_text() {
    let e = env(&[]);
    save(&e, &enabled());
    assert_eq!(
        block_on(e.inline.settings().load()).expect("load"),
        enabled()
    );
    assert_eq!(
        block_on(e.settings.get("ai.inline.enabled"))
            .expect("get")
            .as_deref(),
        Some("true")
    );
    save(&e, &InlineSettings::default());
    assert_eq!(
        block_on(e.inline.settings().load()).expect("load"),
        InlineSettings::default()
    );
}
