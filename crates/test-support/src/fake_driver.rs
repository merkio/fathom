use std::sync::{Arc, Mutex};

use application::ports::{DatabaseDriver, DbError, DbSession, IntrospectScope};
use async_trait::async_trait;
use domain::{ConnectionProfile, Dialect, DriverKind, QueryRequest, QueryResult, SchemaTree};
use secrecy::SecretString;

use crate::lock;

/// Scriptable in-memory database driver.
#[derive(Clone)]
pub struct FakeDriver {
    kind: DriverKind,
    schema: SchemaTree,
    result: QueryResult,
    connect_error: Option<DbError>,
    executed: Arc<Mutex<Vec<String>>>,
    secrets_seen: Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeDriver {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: DriverKind::new(kind),
            schema: SchemaTree::default(),
            result: QueryResult::empty(),
            connect_error: None,
            executed: Arc::default(),
            secrets_seen: Arc::default(),
        }
    }

    pub fn with_schema(mut self, schema: SchemaTree) -> Self {
        self.schema = schema;
        self
    }

    pub fn with_result(mut self, result: QueryResult) -> Self {
        self.result = result;
        self
    }

    pub fn failing_connect(mut self, error: DbError) -> Self {
        self.connect_error = Some(error);
        self
    }

    /// Statements executed through any session of this driver, in order.
    pub fn executed(&self) -> Vec<String> {
        lock(&self.executed).clone()
    }

    /// Whether each `connect` call received a secret. Values are never recorded.
    pub fn secret_presence(&self) -> Vec<bool> {
        lock(&self.secrets_seen)
            .iter()
            .map(Option::is_some)
            .collect()
    }
}

#[async_trait]
impl DatabaseDriver for FakeDriver {
    fn kind(&self) -> DriverKind {
        self.kind.clone()
    }

    fn dialect(&self) -> Dialect {
        Dialect::ANSI
    }

    async fn connect(
        &self,
        _profile: &ConnectionProfile,
        secret: Option<SecretString>,
    ) -> Result<Box<dyn DbSession>, DbError> {
        lock(&self.secrets_seen).push(secret.map(|_| String::new()));
        if let Some(e) = &self.connect_error {
            return Err(e.clone());
        }
        Ok(Box::new(FakeSession {
            schema: self.schema.clone(),
            result: self.result.clone(),
            executed: Arc::clone(&self.executed),
            cancelled: Arc::default(),
        }))
    }
}

pub struct FakeSession {
    schema: SchemaTree,
    result: QueryResult,
    executed: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<Mutex<u32>>,
}

impl FakeSession {
    pub fn cancel_count(&self) -> u32 {
        *lock(&self.cancelled)
    }
}

#[async_trait]
impl DbSession for FakeSession {
    async fn introspect(&self, _scope: IntrospectScope) -> Result<SchemaTree, DbError> {
        Ok(self.schema.clone())
    }

    async fn execute(&self, req: QueryRequest) -> Result<QueryResult, DbError> {
        lock(&self.executed).push(req.text);
        let mut result = self.result.clone();
        // Like a real driver: honour the row limit and say when rows were left out.
        if let Some(limit) = req.row_limit.map(|l| l as usize)
            && result.rows.len() > limit
        {
            result.rows.truncate(limit);
            result.truncated = true;
        }
        Ok(result)
    }

    async fn cancel(&self) -> Result<(), DbError> {
        *lock(&self.cancelled) += 1;
        Ok(())
    }

    async fn close(self: Box<Self>) {}
}

#[cfg(test)]
mod tests {
    use application::registry::DriverRegistry;
    use futures::executor::block_on;
    use std::sync::Arc;

    use super::*;

    fn profile() -> ConnectionProfile {
        ConnectionProfile::new("p", DriverKind::new("fake"), "localhost")
    }

    #[test]
    fn executes_and_records_statements() {
        let driver = FakeDriver::new("fake");
        let session = block_on(driver.connect(&profile(), None)).expect("connect");
        block_on(session.execute(QueryRequest::sql("select 1"))).expect("execute");
        assert_eq!(driver.executed(), vec!["select 1".to_string()]);
        assert_eq!(driver.secret_presence(), vec![false]);
    }

    #[test]
    fn connect_failure_is_scriptable() {
        let driver = FakeDriver::new("fake").failing_connect(DbError::Authentication);
        let err = block_on(driver.connect(&profile(), None)).err();
        assert_eq!(err, Some(DbError::Authentication));
    }

    #[test]
    fn registry_resolves_driver_by_kind() {
        let mut registry = DriverRegistry::new();
        registry.register(Arc::new(FakeDriver::new("fake")));
        assert!(registry.get(&DriverKind::new("fake")).is_some());
        assert!(registry.get(&DriverKind::new("other")).is_none());
        assert_eq!(registry.kinds(), vec![DriverKind::new("fake")]);
    }
}
