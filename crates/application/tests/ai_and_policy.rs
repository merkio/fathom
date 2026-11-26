#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use application::error::UseCaseError;
use application::ports::{DatabaseDriver, LlmError, SecretStore, SettingsRepository};
use application::registry::DriverRegistry;
use application::registry::LlmRegistry;
use application::usecases::{
    AiSettings, AiSettingsStore, ApiKeys, ExecuteOptions, ExecuteQuery, ListModels, OpenSession,
    QueryHistory, SaveConnection,
};
use domain::{
    ConnectionId, ConnectionProfile, Dialect, DriverKind, ModelId, ProviderId, QueryRequest,
    SecretRef,
};
use futures::executor::block_on;
use secrecy::{ExposeSecret, SecretString};
use test_support::{
    FakeDriver, FakeLlm, FixedClock, InMemoryConnections, InMemoryHistory, InMemorySecrets,
    InMemorySettings,
};

fn pg() -> Dialect {
    Dialect {
        name: "postgres",
        ..Dialect::ANSI
    }
}

// ---- read-only mode ----------------------------------------------------------------------

fn guarded_env() -> (
    FakeDriver,
    Box<dyn application::ports::DbSession>,
    ExecuteQuery,
    Arc<InMemoryHistory>,
) {
    let driver = FakeDriver::new("fake");
    let profile = ConnectionProfile::new("p", DriverKind::new("fake"), "h");
    let session = block_on(driver.connect(&profile, None)).expect("connect");
    let history = Arc::new(InMemoryHistory::default());
    let uc = ExecuteQuery::new(
        history.clone(),
        Arc::new(FixedClock(UNIX_EPOCH)),
        ExecuteOptions::default(),
    );
    (driver, session, uc, history)
}

#[test]
fn read_only_mode_blocks_everything_that_is_not_a_plain_read() {
    let (driver, session, uc, history) = guarded_env();
    let id = ConnectionId::new();
    for sql in [
        "INSERT INTO t VALUES (1)",
        "UPDATE t SET a = 1",
        "DELETE FROM t",
        "DROP TABLE t",
        "TRUNCATE t",
        "SELECT 1; DELETE FROM t",
        "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
        "EXPLAIN ANALYZE DELETE FROM t",
        "GRANT ALL ON t TO bob",
        "this is not sql",
        "SELECT * INTO copy FROM t",
    ] {
        let err =
            block_on(uc.execute_guarded(id, session.as_ref(), QueryRequest::sql(sql), &pg(), true))
                .err();
        assert!(
            matches!(err, Some(UseCaseError::Blocked(_))),
            "{sql}: {err:?}"
        );
    }
    assert!(driver.executed().is_empty(), "nothing reached the database");
    assert!(
        block_on(QueryHistory::new(history).recent(id, 10))
            .expect("history")
            .is_empty()
    );
}

#[test]
fn read_only_mode_allows_plain_reads() {
    let (driver, session, uc, _) = guarded_env();
    let id = ConnectionId::new();
    for sql in [
        "SELECT 1",
        "select * from t where a in (select b from u)",
        "SHOW TABLES",
        "EXPLAIN SELECT 1",
        "SELECT 1; SELECT 2",
    ] {
        block_on(uc.execute_guarded(id, session.as_ref(), QueryRequest::sql(sql), &pg(), true))
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    assert_eq!(driver.executed().len(), 5);
}

#[test]
fn the_message_names_what_was_blocked() {
    let (_, session, uc, _) = guarded_env();
    let err = block_on(uc.execute_guarded(
        ConnectionId::new(),
        session.as_ref(),
        QueryRequest::sql("DROP TABLE t"),
        &pg(),
        true,
    ))
    .expect_err("blocked");
    assert!(
        err.to_string().contains("Read-only mode") && err.to_string().contains("ddl"),
        "{err}"
    );
}

#[test]
fn without_read_only_mode_everything_runs() {
    let (driver, session, uc, _) = guarded_env();
    block_on(uc.execute_guarded(
        ConnectionId::new(),
        session.as_ref(),
        QueryRequest::sql("DELETE FROM t"),
        &pg(),
        false,
    ))
    .expect("allowed");
    assert_eq!(driver.executed(), vec!["DELETE FROM t".to_string()]);
}

#[test]
fn open_session_returns_the_profile_so_callers_can_apply_read_only() {
    let repo = Arc::new(InMemoryConnections::default());
    let secrets = Arc::new(InMemorySecrets::default());
    let mut drivers = DriverRegistry::new();
    drivers.register(Arc::new(FakeDriver::new("fake")));
    let mut profile = ConnectionProfile::new("ro", DriverKind::new("fake"), "h");
    profile.set_read_only(true);
    let saved = block_on(SaveConnection::new(repo.clone(), secrets.clone()).execute(profile, None))
        .expect("save");

    let open = OpenSession::new(repo, Arc::new(drivers), secrets);
    let (profile, _session) = block_on(open.execute_with_profile(saved.id)).expect("open");
    assert!(profile.read_only());
}

// ---- AI settings and keys ----------------------------------------------------------------

fn providers() -> Arc<LlmRegistry> {
    let mut r = LlmRegistry::new();
    r.register_with_key(
        Arc::new(FakeLlm::new("cloud", &[]).with_models(&["big", "small"])),
        SecretRef::new("llm/cloud"),
    );
    r.register(Arc::new(FakeLlm::new("local", &[])));
    Arc::new(r)
}

#[test]
fn ai_settings_round_trip_and_default_to_unset() {
    let store = AiSettingsStore::new(Arc::new(InMemorySettings::default()));
    assert_eq!(block_on(store.load()).expect("load"), AiSettings::default());
    let chosen = AiSettings {
        provider: Some(ProviderId::new("local")),
        model: Some(ModelId::new("llama3")),
    };
    block_on(store.save(&chosen)).expect("save");
    assert_eq!(block_on(store.load()).expect("load"), chosen);
    block_on(store.save(&AiSettings::default())).expect("clear");
    assert_eq!(block_on(store.load()).expect("load"), AiSettings::default());
}

#[test]
fn api_keys_go_to_the_secret_store_and_never_to_settings() {
    let settings = Arc::new(InMemorySettings::default());
    let secrets = Arc::new(InMemorySecrets::default());
    let keys = ApiKeys::new(providers(), secrets.clone());
    let cloud = ProviderId::new("cloud");

    assert_eq!(block_on(keys.is_set(&cloud)).expect("check"), Some(false));
    block_on(keys.save(&cloud, SecretString::from("sk-very-secret"))).expect("save");
    assert_eq!(block_on(keys.is_set(&cloud)).expect("check"), Some(true));

    let stored = block_on(secrets.get(&SecretRef::new("llm/cloud")))
        .expect("get")
        .expect("present");
    assert_eq!(stored.expose_secret(), "sk-very-secret");
    // Persisting the AI settings must not carry the key anywhere.
    block_on(AiSettingsStore::new(settings.clone()).save(&AiSettings {
        provider: Some(cloud.clone()),
        model: None,
    }))
    .expect("save");
    for key in ["ai.provider", "ai.model"] {
        let value = block_on(settings.get(key))
            .expect("get")
            .unwrap_or_default();
        assert!(!value.contains("sk-very-secret"));
    }

    block_on(keys.clear(&cloud)).expect("clear");
    assert_eq!(block_on(keys.is_set(&cloud)).expect("check"), Some(false));
}

#[test]
fn providers_without_a_key_report_none_and_reject_key_saving() {
    let keys = ApiKeys::new(providers(), Arc::new(InMemorySecrets::default()));
    let local = ProviderId::new("local");
    assert_eq!(block_on(keys.is_set(&local)).expect("check"), None);
    assert!(matches!(
        block_on(keys.save(&local, SecretString::from("x"))).err(),
        Some(UseCaseError::UnknownProvider(_))
    ));
    assert!(matches!(
        block_on(keys.save(&ProviderId::new("nope"), SecretString::from("x"))).err(),
        Some(UseCaseError::UnknownProvider(_))
    ));
    block_on(keys.clear(&local)).expect("clearing a keyless provider is a no-op");
}

#[test]
fn list_models_asks_the_provider() {
    let list = ListModels::new(providers());
    let models = block_on(list.execute(&ProviderId::new("cloud"))).expect("models");
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["big", "small"]
    );
    assert!(matches!(
        block_on(list.execute(&ProviderId::new("nope"))).err(),
        Some(UseCaseError::UnknownProvider(_))
    ));

    let mut failing = LlmRegistry::new();
    failing.register(Arc::new(
        FakeLlm::new("bad", &[]).failing(LlmError::Authentication),
    ));
    assert_eq!(
        block_on(ListModels::new(Arc::new(failing)).execute(&ProviderId::new("bad"))).err(),
        Some(UseCaseError::Llm(LlmError::Authentication))
    );
}

#[test]
fn registry_knows_which_providers_need_keys() {
    let r = providers();
    assert!(r.key_ref(&ProviderId::new("cloud")).is_some());
    assert!(r.key_ref(&ProviderId::new("local")).is_none());
}
