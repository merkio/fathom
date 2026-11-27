use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

use application::ports::{
    Clock, ConnectionRepository, HistoryEntry, QueryHistoryRepository, SecretError, SecretStore,
    SettingsRepository, StorageError,
};
use async_trait::async_trait;
use domain::{ConnectionId, ConnectionProfile, SecretRef};
use secrecy::SecretString;

use crate::lock;

#[derive(Default)]
pub struct InMemoryConnections {
    items: Mutex<Vec<ConnectionProfile>>,
}

#[async_trait]
impl ConnectionRepository for InMemoryConnections {
    async fn list(&self) -> Result<Vec<ConnectionProfile>, StorageError> {
        Ok(lock(&self.items).clone())
    }

    async fn get(&self, id: ConnectionId) -> Result<ConnectionProfile, StorageError> {
        lock(&self.items)
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or(StorageError::NotFound)
    }

    async fn save(&self, profile: ConnectionProfile) -> Result<(), StorageError> {
        let mut items = lock(&self.items);
        match items.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile,
            None => items.push(profile),
        }
        Ok(())
    }

    async fn delete(&self, id: ConnectionId) -> Result<(), StorageError> {
        lock(&self.items).retain(|p| p.id != id);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryHistory {
    items: Mutex<Vec<HistoryEntry>>,
}

#[async_trait]
impl QueryHistoryRepository for InMemoryHistory {
    async fn append(&self, entry: HistoryEntry) -> Result<(), StorageError> {
        lock(&self.items).push(entry);
        Ok(())
    }

    async fn recent(
        &self,
        connection_id: ConnectionId,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StorageError> {
        Ok(lock(&self.items)
            .iter()
            .rev()
            .filter(|e| e.connection_id == connection_id)
            .take(limit)
            .cloned()
            .collect())
    }
}

#[derive(Default)]
pub struct InMemorySettings {
    items: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl SettingsRepository for InMemorySettings {
    async fn get(&self, key: &str) -> Result<Option<String>, StorageError> {
        Ok(lock(&self.items).get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StorageError> {
        lock(&self.items).insert(key.to_owned(), value.to_owned());
        Ok(())
    }
}

/// Also serves as the in-memory fallback `SecretStore` for headless environments.
#[derive(Default)]
pub struct InMemorySecrets {
    items: Mutex<HashMap<String, SecretString>>,
}

#[async_trait]
impl SecretStore for InMemorySecrets {
    async fn get(&self, key: &SecretRef) -> Result<Option<SecretString>, SecretError> {
        Ok(lock(&self.items).get(key.as_str()).cloned())
    }

    async fn set(&self, key: &SecretRef, value: SecretString) -> Result<(), SecretError> {
        lock(&self.items).insert(key.as_str().to_owned(), value);
        Ok(())
    }

    async fn delete(&self, key: &SecretRef) -> Result<(), SecretError> {
        lock(&self.items).remove(key.as_str());
        Ok(())
    }
}

pub struct FixedClock(pub SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use domain::DriverKind;
    use futures::executor::block_on;
    use secrecy::ExposeSecret;

    use super::*;

    fn profile(name: &str) -> ConnectionProfile {
        ConnectionProfile::new(name, DriverKind::new("fake"), "localhost")
    }

    #[test]
    fn connections_crud() {
        let repo = InMemoryConnections::default();
        let mut p = profile("a");
        block_on(repo.save(p.clone())).expect("save");
        p.name = "renamed".into();
        block_on(repo.save(p.clone())).expect("update");
        assert_eq!(block_on(repo.list()).expect("list").len(), 1);
        assert_eq!(block_on(repo.get(p.id)).expect("get").name, "renamed");
        block_on(repo.delete(p.id)).expect("delete");
        assert_eq!(block_on(repo.get(p.id)), Err(StorageError::NotFound));
    }

    #[test]
    fn history_is_most_recent_first_and_per_connection() {
        let repo = InMemoryHistory::default();
        let (a, b) = (ConnectionId::new(), ConnectionId::new());
        for (id, text) in [(a, "one"), (b, "other"), (a, "two")] {
            let entry = HistoryEntry {
                connection_id: id,
                text: text.into(),
                executed_at: UNIX_EPOCH,
                elapsed: Duration::ZERO,
                succeeded: true,
            };
            block_on(repo.append(entry)).expect("append");
        }
        let recent = block_on(repo.recent(a, 10)).expect("recent");
        assert_eq!(
            recent.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["two", "one"]
        );
        assert_eq!(block_on(repo.recent(a, 1)).expect("recent").len(), 1);
    }

    #[test]
    fn settings_round_trip() {
        let repo = InMemorySettings::default();
        assert_eq!(block_on(repo.get("k")), Ok(None));
        block_on(repo.set("k", "v")).expect("set");
        assert_eq!(block_on(repo.get("k")), Ok(Some("v".to_string())));
    }

    #[test]
    fn secrets_round_trip_and_delete_is_idempotent() {
        let store = InMemorySecrets::default();
        let key = SecretRef::new("k");
        block_on(store.set(&key, SecretString::from("pw"))).expect("set");
        let got = block_on(store.get(&key)).expect("get").expect("present");
        assert_eq!(got.expose_secret(), "pw");
        block_on(store.delete(&key)).expect("delete");
        block_on(store.delete(&key)).expect("delete again");
        assert!(block_on(store.get(&key)).expect("get").is_none());
    }

    #[test]
    fn fixed_clock_is_fixed() {
        assert_eq!(FixedClock(UNIX_EPOCH).now(), UNIX_EPOCH);
    }
}

#[derive(Default)]
pub struct InMemorySavedQueries {
    items: Mutex<Vec<domain::SavedQuery>>,
}

#[async_trait]
impl application::ports::SavedQueryRepository for InMemorySavedQueries {
    async fn list(&self) -> Result<Vec<domain::SavedQuery>, StorageError> {
        let mut all = lock(&self.items).clone();
        all.sort_by_key(|q| q.name.to_lowercase());
        Ok(all)
    }

    async fn save(&self, query: domain::SavedQuery) -> Result<(), StorageError> {
        let mut items = lock(&self.items);
        items.retain(|q| q.id != query.id);
        items.push(query);
        Ok(())
    }

    async fn delete(&self, id: domain::SavedQueryId) -> Result<(), StorageError> {
        lock(&self.items).retain(|q| q.id != id);
        Ok(())
    }
}
