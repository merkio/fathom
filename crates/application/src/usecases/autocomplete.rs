//! Local, schema-aware SQL autocomplete.
//!
//! Pure and synchronous: it works over an already loaded `SchemaTree`, never touches the network
//! or the database, and is fast enough to run on every keystroke.

use crate::sql::{CompletionRequest, Suggestion, complete};

#[derive(Debug, Default, Clone, Copy)]
pub struct CompleteSql;

impl CompleteSql {
    pub fn execute(&self, request: &CompletionRequest<'_>) -> Vec<Suggestion> {
        complete(request)
    }
}
