#![allow(clippy::expect_used)] // test helpers

use std::sync::Arc;

use application::error::UseCaseError;
use application::ports::{ConnectionRepository, DbError, SecretStore, StorageError};
use application::registry::DriverRegistry;
use application::usecases::{
    CloseSession, DeleteConnection, ListConnections, OpenSession, SaveConnection, TestConnection,
};
use domain::{ConnectionId, ConnectionProfile, DomainError, DriverKind, SecretRef};
use futures::executor::block_on;
use secrecy::{ExposeSecret, SecretString};
use test_support::{FakeDriver, InMemoryConnections, InMemorySecrets};

fn profile(name: &str) -> ConnectionProfile {
    ConnectionProfile::new(name, DriverKind::new("fake"), "localhost")
}

fn pw(s: &str) -> Option<SecretString> {
    Some(SecretString::from(s))
}

struct Env {
    repo: Arc<InMemoryConnections>,
    secrets: Arc<InMemorySecrets>,
}

fn env() -> Env {
    Env {
        repo: Arc::default(),
        secrets: Arc::default(),
    }
}

fn registry(driver: &FakeDriver) -> Arc<DriverRegistry> {
    let mut r = DriverRegistry::new();
    r.register(Arc::new(driver.clone()));
    Arc::new(r)
}

#[test]
fn save_stores_password_in_secret_store_only() {
    let e = env();
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    let saved = block_on(save.execute(profile("db"), pw("hunter2"))).expect("save");

    let key = SecretRef::for_connection(saved.id);
    assert_eq!(saved.secret_ref, Some(key.clone()));
    let stored = block_on(e.secrets.get(&key))
        .expect("get")
        .expect("secret present");
    assert_eq!(stored.expose_secret(), "hunter2");
    // The persisted profile has only the reference.
    let persisted = block_on(e.repo.get(saved.id)).expect("profile");
    assert_eq!(persisted.secret_ref, Some(key));
}

#[test]
fn save_without_password_keeps_existing_secret() {
    let e = env();
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    let saved = block_on(save.execute(profile("db"), pw("first"))).expect("save");
    let mut edited = saved.clone();
    edited.name = "renamed".into();
    block_on(save.execute(edited, None)).expect("resave");

    let key = SecretRef::for_connection(saved.id);
    let stored = block_on(e.secrets.get(&key))
        .expect("get")
        .expect("still there");
    assert_eq!(stored.expose_secret(), "first");
}

#[test]
fn invalid_profile_is_rejected_and_nothing_is_stored() {
    let e = env();
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    let mut bad = profile("db");
    bad.host.clear();
    let err = block_on(save.execute(bad.clone(), pw("secret"))).err();
    assert!(matches!(
        err,
        Some(UseCaseError::Domain(DomainError::Invalid {
            field: "host",
            ..
        }))
    ));
    assert!(block_on(e.repo.list()).expect("list").is_empty());
    assert!(
        block_on(e.secrets.get(&SecretRef::for_connection(bad.id)))
            .expect("get")
            .is_none()
    );
}

#[test]
fn list_is_sorted_by_name_ignoring_case() {
    let e = env();
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    for name in ["beta", "Alpha", "gamma"] {
        block_on(save.execute(profile(name), None)).expect("save");
    }
    let names: Vec<_> = block_on(ListConnections::new(e.repo.clone()).execute())
        .expect("list")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["Alpha", "beta", "gamma"]);
}

#[test]
fn delete_removes_profile_and_secret_and_is_idempotent() {
    let e = env();
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    let saved = block_on(save.execute(profile("db"), pw("pw"))).expect("save");
    let delete = DeleteConnection::new(e.repo.clone(), e.secrets.clone());

    block_on(delete.execute(saved.id)).expect("delete");
    assert_eq!(block_on(e.repo.get(saved.id)), Err(StorageError::NotFound));
    assert!(
        block_on(e.secrets.get(&SecretRef::for_connection(saved.id)))
            .expect("get")
            .is_none()
    );
    block_on(delete.execute(saved.id)).expect("second delete is fine");
}

#[test]
fn test_connection_uses_supplied_password_without_saving() {
    let e = env();
    let driver = FakeDriver::new("fake");
    let test = TestConnection::new(registry(&driver), e.secrets.clone());
    block_on(test.execute(&profile("db"), pw("typed"))).expect("test");
    assert_eq!(driver.secret_presence(), vec![true]);
    assert!(block_on(e.repo.list()).expect("list").is_empty());
}

#[test]
fn test_connection_falls_back_to_stored_secret() {
    let e = env();
    let driver = FakeDriver::new("fake");
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    let saved = block_on(save.execute(profile("db"), pw("stored"))).expect("save");
    let test = TestConnection::new(registry(&driver), e.secrets.clone());
    block_on(test.execute(&saved, None)).expect("test");
    assert_eq!(driver.secret_presence(), vec![true]);
}

#[test]
fn test_connection_reports_unknown_driver_and_driver_errors() {
    let e = env();
    let empty = Arc::new(DriverRegistry::new());
    let test = TestConnection::new(empty, e.secrets.clone());
    let err = block_on(test.execute(&profile("db"), None)).err();
    assert_eq!(
        err,
        Some(UseCaseError::UnknownDriver(DriverKind::new("fake")))
    );

    let driver = FakeDriver::new("fake").failing_connect(DbError::Authentication);
    let test = TestConnection::new(registry(&driver), e.secrets.clone());
    let err = block_on(test.execute(&profile("db"), None)).err();
    assert_eq!(err, Some(UseCaseError::Db(DbError::Authentication)));
}

#[test]
fn open_session_connects_with_stored_secret_and_can_run_queries() {
    let e = env();
    let driver = FakeDriver::new("fake");
    let save = SaveConnection::new(e.repo.clone(), e.secrets.clone());
    let saved = block_on(save.execute(profile("db"), pw("pw"))).expect("save");

    let open = OpenSession::new(e.repo.clone(), registry(&driver), e.secrets.clone());
    let session = block_on(open.execute(saved.id)).expect("open");
    assert_eq!(driver.secret_presence(), vec![true]);
    block_on(session.execute(domain::QueryRequest::sql("select 1"))).expect("query");
    block_on(CloseSession.execute(session));
    assert_eq!(driver.executed(), vec!["select 1".to_string()]);
}

#[test]
fn open_session_for_unknown_connection_fails() {
    let e = env();
    let open = OpenSession::new(
        e.repo.clone(),
        registry(&FakeDriver::new("fake")),
        e.secrets.clone(),
    );
    let id = ConnectionId::new();
    assert_eq!(
        block_on(open.execute(id)).err(),
        Some(UseCaseError::ConnectionNotFound(id))
    );
}
