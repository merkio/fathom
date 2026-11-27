//! Saving, listing and deleting named queries.

use std::sync::Arc;

use domain::{ConnectionId, SavedQuery, SavedQueryId};

use crate::error::UseCaseError;
use crate::ports::{Clock, SavedQueryRepository};

const MAX_NAME_CHARS: usize = 100;

/// What the user typed when saving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDraft {
    /// `Some` to update an existing saved query (rename or new text), `None` for a new one.
    pub id: Option<SavedQueryId>,
    pub name: String,
    pub text: String,
    pub connection: Option<ConnectionId>,
}

pub struct SavedQueries {
    repo: Arc<dyn SavedQueryRepository>,
    clock: Arc<dyn Clock>,
}

impl SavedQueries {
    pub fn new(repo: Arc<dyn SavedQueryRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, clock }
    }

    pub async fn list(&self) -> Result<Vec<SavedQuery>, UseCaseError> {
        Ok(self.repo.list().await?)
    }

    /// Validates and stores. Names are unique (ignoring case) so the sidebar is unambiguous.
    pub async fn save(&self, draft: SaveDraft) -> Result<SavedQuery, UseCaseError> {
        let name = draft.name.trim().to_owned();
        if name.is_empty() {
            return Err(UseCaseError::Blocked("Give the query a name.".into()));
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(UseCaseError::Blocked(format!(
                "The name is too long (at most {MAX_NAME_CHARS} characters)."
            )));
        }
        if name.chars().any(char::is_control) {
            return Err(UseCaseError::Blocked(
                "The name contains an invalid character.".into(),
            ));
        }
        if draft.text.trim().is_empty() {
            return Err(UseCaseError::Blocked("There is no SQL to save.".into()));
        }
        let id = draft.id.unwrap_or_default();
        let clash = self
            .repo
            .list()
            .await?
            .into_iter()
            .any(|q| q.id != id && q.name.to_lowercase() == name.to_lowercase());
        if clash {
            return Err(UseCaseError::Blocked(format!(
                "A saved query named `{name}` already exists."
            )));
        }
        let query = SavedQuery {
            id,
            name,
            text: draft.text,
            connection: draft.connection,
            updated_at: self.clock.now(),
        };
        self.repo.save(query.clone()).await?;
        Ok(query)
    }

    pub async fn delete(&self, id: SavedQueryId) -> Result<(), UseCaseError> {
        Ok(self.repo.delete(id).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use async_trait::async_trait;
    use futures::executor::block_on;

    use super::*;
    use crate::ports::StorageError;

    #[derive(Default)]
    struct Memory(Mutex<Vec<SavedQuery>>);

    #[async_trait]
    impl SavedQueryRepository for Memory {
        async fn list(&self) -> Result<Vec<SavedQuery>, StorageError> {
            let mut all = self.0.lock().unwrap().clone();
            all.sort_by_key(|q| q.name.to_lowercase());
            Ok(all)
        }
        async fn save(&self, query: SavedQuery) -> Result<(), StorageError> {
            let mut items = self.0.lock().unwrap();
            items.retain(|q| q.id != query.id);
            items.push(query);
            Ok(())
        }
        async fn delete(&self, id: SavedQueryId) -> Result<(), StorageError> {
            self.0.lock().unwrap().retain(|q| q.id != id);
            Ok(())
        }
    }

    struct Fixed;

    fn fixed_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    impl Clock for Fixed {
        fn now(&self) -> SystemTime {
            fixed_time()
        }
    }

    fn uc() -> SavedQueries {
        SavedQueries::new(Arc::new(Memory::default()), Arc::new(Fixed))
    }

    fn draft(name: &str, text: &str) -> SaveDraft {
        SaveDraft {
            id: None,
            name: name.into(),
            text: text.into(),
            connection: None,
        }
    }

    fn blocked(r: Result<SavedQuery, UseCaseError>) -> String {
        match r {
            Err(UseCaseError::Blocked(m)) => m,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_saved_query_is_listed_by_name_ignoring_case() {
        let uc = uc();
        block_on(uc.save(draft("revenue", "select 1"))).unwrap();
        block_on(uc.save(draft("Active users", "select 2"))).unwrap();
        block_on(uc.save(draft("customers", "select 3"))).unwrap();
        let names: Vec<_> = block_on(uc.list())
            .unwrap()
            .into_iter()
            .map(|q| q.name)
            .collect();
        assert_eq!(names, ["Active users", "customers", "revenue"]);
    }

    #[test]
    fn names_are_trimmed_and_validated() {
        let uc = uc();
        assert_eq!(
            block_on(uc.save(draft("  spaced  ", "select 1")))
                .unwrap()
                .name,
            "spaced"
        );
        assert!(blocked(block_on(uc.save(draft("   ", "select 1")))).contains("name"));
        assert!(
            blocked(block_on(uc.save(draft(&"x".repeat(101), "select 1")))).contains("too long")
        );
        assert!(blocked(block_on(uc.save(draft("bad\nname", "select 1")))).contains("invalid"));
        assert!(blocked(block_on(uc.save(draft("empty", "  \n")))).contains("no SQL"));
        assert_eq!(
            block_on(uc.list()).unwrap().len(),
            1,
            "refused drafts store nothing"
        );
    }

    #[test]
    fn a_new_query_cannot_take_an_existing_name_but_an_update_can_keep_its_own() {
        let uc = uc();
        let first = block_on(uc.save(draft("Report", "select 1"))).unwrap();
        assert!(blocked(block_on(uc.save(draft("report", "select 2")))).contains("already exists"));
        let again = block_on(uc.save(SaveDraft {
            id: Some(first.id),
            name: "REPORT".into(),
            text: "select 3".into(),
            connection: None,
        }))
        .unwrap();
        assert_eq!((again.id, again.text.as_str()), (first.id, "select 3"));
        assert_eq!(
            block_on(uc.list()).unwrap().len(),
            1,
            "updated, not duplicated"
        );
    }

    #[test]
    fn renaming_to_another_queries_name_is_refused() {
        let uc = uc();
        block_on(uc.save(draft("a", "select 1"))).unwrap();
        let b = block_on(uc.save(draft("b", "select 2"))).unwrap();
        let r = block_on(uc.save(SaveDraft {
            id: Some(b.id),
            name: "A".into(),
            text: "select 2".into(),
            connection: None,
        }));
        assert!(blocked(r).contains("already exists"));
    }

    #[test]
    fn delete_removes_only_that_query() {
        let uc = uc();
        let a = block_on(uc.save(draft("a", "select 1"))).unwrap();
        block_on(uc.save(draft("b", "select 2"))).unwrap();
        block_on(uc.delete(a.id)).unwrap();
        let names: Vec<_> = block_on(uc.list())
            .unwrap()
            .into_iter()
            .map(|q| q.name)
            .collect();
        assert_eq!(names, ["b"]);
        block_on(uc.delete(a.id)).unwrap(); // deleting twice is harmless
    }

    #[test]
    fn the_save_time_comes_from_the_clock_and_the_connection_is_kept() {
        let uc = uc();
        let conn = ConnectionId::new();
        let q = block_on(uc.save(SaveDraft {
            connection: Some(conn),
            ..draft("c", "select 1")
        }))
        .unwrap();
        assert_eq!(q.connection, Some(conn));
        assert_eq!(q.updated_at, fixed_time());
    }
}
