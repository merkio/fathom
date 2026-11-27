//! Which databases (schemas) of each instance appear in the explorer.
//!
//! Stored per connection in the key/value settings as one name per line. No entry means "not
//! chosen yet": everything is shown, so existing connections keep working. An explicit empty
//! choice (hide everything) is stored as a marker line so it can be told apart from "no entry".

use std::sync::Arc;

use domain::ConnectionId;

use crate::ports::{SettingsRepository, StorageError};

const PREFIX: &str = "ui.explorer.visible.";
/// Stored when the user chose to show nothing; schema names cannot contain a newline.
const NOTHING: &str = "\n";

fn key(connection: ConnectionId) -> String {
    format!("{PREFIX}{}", connection.0)
}

pub struct ExplorerVisibilityStore {
    settings: Arc<dyn SettingsRepository>,
}

impl ExplorerVisibilityStore {
    pub fn new(settings: Arc<dyn SettingsRepository>) -> Self {
        Self { settings }
    }

    /// The names the user chose to show, or `None` when nothing was chosen yet.
    pub async fn load(
        &self,
        connection: ConnectionId,
    ) -> Result<Option<Vec<String>>, StorageError> {
        Ok(self
            .settings
            .get(&key(connection))
            .await?
            .map(|text| decode(&text)))
    }

    pub async fn save(
        &self,
        connection: ConnectionId,
        visible: &[String],
    ) -> Result<(), StorageError> {
        self.settings.set(&key(connection), &encode(visible)).await
    }
}

fn encode(visible: &[String]) -> String {
    if visible.is_empty() {
        NOTHING.to_owned()
    } else {
        visible.join("\n")
    }
}

fn decode(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures::executor::block_on;

    use super::*;

    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);

    #[async_trait]
    impl SettingsRepository for Memory {
        async fn get(&self, key: &str) -> Result<Option<String>, StorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &str) -> Result<(), StorageError> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
    }

    fn store() -> ExplorerVisibilityStore {
        ExplorerVisibilityStore::new(Arc::new(Memory::default()))
    }

    #[test]
    fn nothing_chosen_yet_is_none() {
        assert_eq!(block_on(store().load(ConnectionId::new())).unwrap(), None);
    }

    #[test]
    fn a_choice_round_trips_in_order() {
        let s = store();
        let id = ConnectionId::new();
        let names = vec!["commerce".to_owned(), "analytics".to_owned()];
        block_on(s.save(id, &names)).unwrap();
        assert_eq!(block_on(s.load(id)).unwrap(), Some(names));
    }

    #[test]
    fn choosing_nothing_is_not_the_same_as_never_choosing() {
        let s = store();
        let id = ConnectionId::new();
        block_on(s.save(id, &[])).unwrap();
        assert_eq!(block_on(s.load(id)).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn instances_do_not_share_choices() {
        let s = store();
        let (a, b) = (ConnectionId::new(), ConnectionId::new());
        block_on(s.save(a, &["x".to_owned()])).unwrap();
        assert_eq!(block_on(s.load(b)).unwrap(), None);
    }
}
