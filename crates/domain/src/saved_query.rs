//! Saved queries: SQL the user chose to keep, with a name.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ConnectionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SavedQueryId(pub Uuid);

impl SavedQueryId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SavedQueryId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedQuery {
    pub id: SavedQueryId,
    pub name: String,
    pub text: String,
    /// The connection it was written for (a hint for where to open it), if any.
    pub connection: Option<ConnectionId>,
    pub updated_at: SystemTime,
}
