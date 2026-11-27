use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use application::ports::{
    ConnectionRepository, HistoryEntry, QueryHistoryRepository, SavedQueryRepository, SecretStore,
    SettingsRepository, StorageError,
};
use application::usecases::SaveConnection;
use domain::{ConnectionId, ConnectionProfile, DriverKind, SavedQuery, SavedQueryId, SecretRef};
use futures::executor::block_on;
use secrecy::{ExposeSecret, SecretString};
use storage::{MemorySecretStore, SqliteStore};

fn profile(name: &str) -> ConnectionProfile {
    let mut p = ConnectionProfile::new(name, DriverKind::new("postgres"), "db.local");
    p.port = Some(5432);
    p.database = Some("app".into());
    p.username = "alice".into();
    p.options.insert("sslmode".into(), "require".into());
    p
}

#[test]
fn connection_crud_round_trips_every_field() {
    let store = SqliteStore::open_in_memory().expect("open");
    let mut p = profile("main");
    p.secret_ref = Some(SecretRef::for_connection(p.id));
    block_on(ConnectionRepository::save(&store, p.clone())).expect("save");
    assert_eq!(
        block_on(ConnectionRepository::get(&store, p.id)),
        Ok(p.clone())
    );

    p.name = "renamed".into();
    p.options.clear();
    p.options.insert("timeout".into(), "5".into());
    block_on(ConnectionRepository::save(&store, p.clone())).expect("update");
    assert_eq!(
        block_on(ConnectionRepository::list(&store)).expect("list"),
        vec![p.clone()]
    );

    block_on(ConnectionRepository::delete(&store, p.id)).expect("delete");
    assert_eq!(
        block_on(ConnectionRepository::get(&store, p.id)),
        Err(StorageError::NotFound)
    );
    block_on(ConnectionRepository::delete(&store, p.id)).expect("delete missing is fine");
}

#[test]
fn data_survives_reopening_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fathom.sqlite");
    let p = profile("persisted");
    {
        let store = SqliteStore::open(&path).expect("open");
        block_on(ConnectionRepository::save(&store, p.clone())).expect("save");
        block_on(SettingsRepository::set(&store, "theme", "dark")).expect("setting");
    }
    let store = SqliteStore::open(&path).expect("reopen");
    assert_eq!(block_on(ConnectionRepository::get(&store, p.id)), Ok(p));
    assert_eq!(
        block_on(SettingsRepository::get(&store, "theme")),
        Ok(Some("dark".into()))
    );
}

#[test]
fn refuses_databases_from_a_newer_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("future.sqlite");
    rusqlite::Connection::open(&path)
        .and_then(|c| c.execute_batch("PRAGMA user_version = 99;"))
        .expect("prepare file");
    assert!(SqliteStore::open(&path).is_err());
}

#[test]
fn history_is_per_connection_most_recent_first() {
    let store = SqliteStore::open_in_memory().expect("open");
    let (a, b) = (ConnectionId::new(), ConnectionId::new());
    for (id, text, ok) in [(a, "one", true), (b, "other", true), (a, "two", false)] {
        let entry = HistoryEntry {
            connection_id: id,
            text: text.into(),
            executed_at: UNIX_EPOCH + Duration::from_secs(10),
            elapsed: Duration::from_millis(42),
            succeeded: ok,
        };
        block_on(store.append(entry)).expect("append");
    }
    let recent = block_on(store.recent(a, 10)).expect("recent");
    assert_eq!(
        recent.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        ["two", "one"]
    );
    assert_eq!(recent[0].elapsed, Duration::from_millis(42));
    assert_eq!(recent[0].executed_at, UNIX_EPOCH + Duration::from_secs(10));
    assert!(!recent[0].succeeded && recent[1].succeeded);
    assert_eq!(block_on(store.recent(a, 1)).expect("limit").len(), 1);
}

#[test]
fn settings_upsert() {
    let store = SqliteStore::open_in_memory().expect("open");
    assert_eq!(block_on(SettingsRepository::get(&store, "k")), Ok(None));
    block_on(SettingsRepository::set(&store, "k", "1")).expect("set");
    block_on(SettingsRepository::set(&store, "k", "2")).expect("overwrite");
    assert_eq!(
        block_on(SettingsRepository::get(&store, "k")),
        Ok(Some("2".into()))
    );
}

#[test]
fn memory_secret_store_round_trip() {
    let store = MemorySecretStore::new();
    let key = SecretRef::new("k");
    block_on(store.set(&key, SecretString::from("pw"))).expect("set");
    let got = block_on(store.get(&key)).expect("get").expect("present");
    assert_eq!(got.expose_secret(), "pw");
    block_on(store.delete(&key)).expect("delete");
    block_on(store.delete(&key)).expect("idempotent");
    assert!(block_on(store.get(&key)).expect("get").is_none());
}

/// M2 acceptance: no password appears in the SQLite file (including its WAL/journal siblings).
#[test]
fn no_password_is_written_to_the_sqlite_file() {
    const PASSWORD: &str = "correct-horse-battery-staple-7f3a";
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fathom.sqlite");
    let store = Arc::new(SqliteStore::open(&path).expect("open"));
    let secrets = Arc::new(MemorySecretStore::new());

    let save = SaveConnection::new(store.clone(), secrets.clone());
    let saved =
        block_on(save.execute(profile("db"), Some(SecretString::from(PASSWORD)))).expect("save");
    drop(store);

    let mut scanned = 0;
    for entry in std::fs::read_dir(dir.path()).expect("read dir") {
        let bytes = std::fs::read(entry.expect("entry").path()).expect("read file");
        scanned += 1;
        assert!(
            !bytes
                .windows(PASSWORD.len())
                .any(|w| w == PASSWORD.as_bytes()),
            "password found in database files"
        );
    }
    assert!(scanned >= 1);
    // The secret is retrievable only through the secret store.
    let key = saved.secret_ref.expect("secret ref set");
    let stored = block_on(secrets.get(&key)).expect("get").expect("present");
    assert_eq!(stored.expose_secret(), PASSWORD);
}

/// Real OS keychain; needs a desktop session, so it is opt-in: `cargo test -- --ignored`.
#[test]
#[ignore = "requires an OS keychain"]
fn keyring_store_round_trip() {
    let store = storage::KeyringSecretStore::new("fathom-test");
    let key = SecretRef::new("round-trip");
    block_on(store.set(&key, SecretString::from("pw"))).expect("set");
    let got = block_on(store.get(&key)).expect("get").expect("present");
    assert_eq!(got.expose_secret(), "pw");
    block_on(store.delete(&key)).expect("delete");
    assert!(block_on(store.get(&key)).expect("get").is_none());
}

fn saved(name: &str, text: &str) -> SavedQuery {
    SavedQuery {
        id: SavedQueryId::new(),
        name: name.into(),
        text: text.into(),
        connection: Some(ConnectionId::new()),
        updated_at: UNIX_EPOCH + Duration::from_millis(1_700_000_000_000),
    }
}

#[test]
fn saved_queries_round_trip_in_name_order_and_upsert_by_id() {
    let store = SqliteStore::open_in_memory().expect("open");
    let (b, a) = (saved("beta", "select 2"), saved("Alpha", "select 1"));
    block_on(SavedQueryRepository::save(&store, b.clone())).expect("save b");
    block_on(SavedQueryRepository::save(&store, a.clone())).expect("save a");
    let list = block_on(SavedQueryRepository::list(&store)).expect("list");
    assert_eq!(
        list,
        vec![a.clone(), b.clone()],
        "ordered by name ignoring case, all fields kept"
    );

    let renamed = SavedQuery {
        name: "gamma".into(),
        text: "select 3".into(),
        connection: None,
        ..a.clone()
    };
    block_on(SavedQueryRepository::save(&store, renamed.clone())).expect("update");
    let list = block_on(SavedQueryRepository::list(&store)).expect("list");
    assert_eq!(
        list,
        vec![b.clone(), renamed],
        "updated in place, not duplicated"
    );

    block_on(SavedQueryRepository::delete(&store, b.id)).expect("delete");
    block_on(SavedQueryRepository::delete(&store, b.id)).expect("deleting twice is fine");
    assert_eq!(
        block_on(SavedQueryRepository::list(&store))
            .expect("list")
            .len(),
        1
    );
}

#[test]
fn the_database_itself_refuses_two_queries_with_the_same_name() {
    let store = SqliteStore::open_in_memory().expect("open");
    block_on(SavedQueryRepository::save(
        &store,
        saved("Report", "select 1"),
    ))
    .expect("first");
    let clash = block_on(SavedQueryRepository::save(
        &store,
        saved("REPORT", "select 2"),
    ));
    assert!(
        clash.is_err(),
        "case-insensitive uniqueness is enforced below the use case too"
    );
}

#[test]
fn saved_queries_survive_reopening_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("fathom.sqlite");
    let q = saved("keep", "select 1");
    {
        let store = SqliteStore::open(&path).expect("open");
        block_on(SavedQueryRepository::save(&store, q.clone())).expect("save");
    }
    let store = SqliteStore::open(&path).expect("reopen");
    assert_eq!(
        block_on(SavedQueryRepository::list(&store)).expect("list"),
        vec![q]
    );
}

#[test]
fn a_version_1_database_upgrades_in_place_without_losing_data() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.sqlite");
    let id = ConnectionId::new();
    {
        // The schema exactly as version 1 created it, with a connection and a setting in it.
        let c = rusqlite::Connection::open(&path).expect("create");
        c.execute_batch(
            "CREATE TABLE connections (id TEXT PRIMARY KEY, name TEXT NOT NULL, driver_kind TEXT NOT NULL,
                 host TEXT NOT NULL, port INTEGER, database TEXT, username TEXT NOT NULL, secret_ref TEXT);
             CREATE TABLE connection_options (connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
                 key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (connection_id, key));
             CREATE TABLE query_history (seq INTEGER PRIMARY KEY AUTOINCREMENT, connection_id TEXT NOT NULL,
                 text TEXT NOT NULL, executed_at_ms INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, succeeded INTEGER NOT NULL);
             CREATE INDEX query_history_by_connection ON query_history(connection_id, seq);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             PRAGMA user_version = 1;",
        )
        .expect("v1 schema");
        c.execute(
            "INSERT INTO connections (id, name, driver_kind, host, username) VALUES (?1, 'old', 'postgres', 'h', 'u')",
            [id.0.to_string()],
        )
        .expect("connection");
        c.execute("INSERT INTO settings VALUES ('theme', 'dark')", [])
            .expect("setting");
    }
    let store = SqliteStore::open(&path).expect("upgrade");
    assert_eq!(
        block_on(ConnectionRepository::list(&store))
            .expect("connections")
            .len(),
        1,
        "old data is kept"
    );
    assert_eq!(
        block_on(SettingsRepository::get(&store, "theme")),
        Ok(Some("dark".into()))
    );
    block_on(SavedQueryRepository::save(&store, saved("new", "select 1")))
        .expect("the new table works");
    drop(store);
    let version: i64 = rusqlite::Connection::open(&path)
        .and_then(|c| c.query_row("PRAGMA user_version", [], |r| r.get(0)))
        .expect("version");
    assert_eq!(version, 2);
    // Opening again must not try to migrate twice.
    SqliteStore::open(&path).expect("reopen after upgrade");
}
