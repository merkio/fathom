use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use application::ports::{
    ConnectionRepository, HistoryEntry, QueryHistoryRepository, SettingsRepository, StorageError,
};
use async_trait::async_trait;
use domain::{ConnectionId, ConnectionProfile, DriverKind, SecretRef};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

/// Current schema version, stored in `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 1;

const MIGRATION_V1: &str = "
CREATE TABLE connections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    driver_kind TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER,
    database TEXT,
    username TEXT NOT NULL,
    secret_ref TEXT
);
CREATE TABLE connection_options (
    connection_id TEXT NOT NULL REFERENCES connections(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (connection_id, key)
);
CREATE TABLE query_history (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    connection_id TEXT NOT NULL,
    text TEXT NOT NULL,
    executed_at_ms INTEGER NOT NULL,
    elapsed_ms INTEGER NOT NULL,
    succeeded INTEGER NOT NULL
);
CREATE INDEX query_history_by_connection ON query_history(connection_id, seq);
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// SQLite-backed connection, history and settings repositories (one database file).
///
/// Operations are short and synchronous; use cases run on the background runtime.
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

fn backend(e: rusqlite::Error) -> StorageError {
    StorageError::Backend(e.to_string())
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::from_connection(Connection::open(path).map_err(backend)?)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        Self::from_connection(Connection::open_in_memory().map_err(backend)?)
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(backend)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(backend)?;
        if version > SCHEMA_VERSION {
            return Err(StorageError::Backend(format!(
                "database schema version {version} is newer than supported {SCHEMA_VERSION}"
            )));
        }
        if version < 1 {
            conn.execute_batch(&format!(
                "BEGIN; {MIGRATION_V1} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
            ))
            .map_err(backend)?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn parse_id(s: &str) -> Result<ConnectionId, StorageError> {
    Uuid::parse_str(s)
        .map(ConnectionId)
        .map_err(|e| StorageError::Backend(format!("invalid connection id in database: {e}")))
}

fn load_profile(conn: &Connection, id: &str) -> Result<Option<ConnectionProfile>, StorageError> {
    let row = conn
        .query_row(
            "SELECT id, name, driver_kind, host, port, database, username, secret_ref
             FROM connections WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<u16>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(backend)?;
    let Some((id, name, kind, host, port, database, username, secret_ref)) = row else {
        return Ok(None);
    };
    let mut stmt = conn
        .prepare("SELECT key, value FROM connection_options WHERE connection_id = ?1")
        .map_err(backend)?;
    let options = stmt
        .query_map([&id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(backend)?
        .collect::<Result<_, _>>()
        .map_err(backend)?;
    Ok(Some(ConnectionProfile {
        id: parse_id(&id)?,
        name,
        driver_kind: DriverKind::new(kind),
        host,
        port,
        database,
        username,
        secret_ref: secret_ref.map(SecretRef::new),
        options,
    }))
}

#[async_trait]
impl ConnectionRepository for SqliteStore {
    async fn list(&self) -> Result<Vec<ConnectionProfile>, StorageError> {
        let conn = self.conn();
        let ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM connections")
                .map_err(backend)?;
            stmt.query_map([], |r| r.get(0))
                .map_err(backend)?
                .collect::<Result<_, _>>()
                .map_err(backend)?
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(p) = load_profile(&conn, &id)? {
                out.push(p);
            }
        }
        Ok(out)
    }

    async fn get(&self, id: ConnectionId) -> Result<ConnectionProfile, StorageError> {
        load_profile(&self.conn(), &id.0.to_string())?.ok_or(StorageError::NotFound)
    }

    async fn save(&self, profile: ConnectionProfile) -> Result<(), StorageError> {
        let mut conn = self.conn();
        let tx = conn.transaction().map_err(backend)?;
        let id = profile.id.0.to_string();
        tx.execute(
            "INSERT INTO connections
                 (id, name, driver_kind, host, port, database, username, secret_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 name = ?2, driver_kind = ?3, host = ?4, port = ?5,
                 database = ?6, username = ?7, secret_ref = ?8",
            params![
                id,
                profile.name,
                profile.driver_kind.as_str(),
                profile.host,
                profile.port,
                profile.database,
                profile.username,
                profile.secret_ref.as_ref().map(SecretRef::as_str),
            ],
        )
        .map_err(backend)?;
        tx.execute(
            "DELETE FROM connection_options WHERE connection_id = ?1",
            [&id],
        )
        .map_err(backend)?;
        for (k, v) in &profile.options {
            tx.execute(
                "INSERT INTO connection_options (connection_id, key, value) VALUES (?1, ?2, ?3)",
                params![id, k, v],
            )
            .map_err(backend)?;
        }
        tx.commit().map_err(backend)
    }

    async fn delete(&self, id: ConnectionId) -> Result<(), StorageError> {
        self.conn()
            .execute("DELETE FROM connections WHERE id = ?1", [id.0.to_string()])
            .map_err(backend)?;
        Ok(())
    }
}

fn to_millis(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn from_millis(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}

#[async_trait]
impl QueryHistoryRepository for SqliteStore {
    async fn append(&self, entry: HistoryEntry) -> Result<(), StorageError> {
        self.conn()
            .execute(
                "INSERT INTO query_history
                     (connection_id, text, executed_at_ms, elapsed_ms, succeeded)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    entry.connection_id.0.to_string(),
                    entry.text,
                    to_millis(entry.executed_at),
                    i64::try_from(entry.elapsed.as_millis()).unwrap_or(i64::MAX),
                    entry.succeeded,
                ],
            )
            .map_err(backend)?;
        Ok(())
    }

    async fn recent(
        &self,
        connection_id: ConnectionId,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StorageError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT text, executed_at_ms, elapsed_ms, succeeded FROM query_history
                 WHERE connection_id = ?1 ORDER BY seq DESC LIMIT ?2",
            )
            .map_err(backend)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt
            .query_map(params![connection_id.0.to_string(), limit], |r| {
                Ok(HistoryEntry {
                    connection_id,
                    text: r.get(0)?,
                    executed_at: from_millis(r.get(1)?),
                    elapsed: Duration::from_millis(u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0)),
                    succeeded: r.get(3)?,
                })
            })
            .map_err(backend)?
            .collect::<Result<_, _>>()
            .map_err(backend)?;
        Ok(rows)
    }
}

#[async_trait]
impl SettingsRepository for SqliteStore {
    async fn get(&self, key: &str) -> Result<Option<String>, StorageError> {
        self.conn()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(backend)
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        self.conn()
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, value],
            )
            .map_err(backend)?;
        Ok(())
    }
}
