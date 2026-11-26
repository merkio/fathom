//! SQLite adapter: implements `DatabaseDriver` for local database files.
//!
//! This crate exists partly as the architecture's extensibility proof: a new database is one
//! crate that implements the port plus one registration line in `app`.
//!
//! Connection mapping: the database file path is the profile's `database`; `host`, `port`,
//! `username` and the password are ignored (SQLite has no server or authentication). Options:
//! `mode=ro` opens the file read-only, `create=true` creates it if missing (off by default so a
//! typo in the path is an error, not a new empty database).

mod driver;
mod introspect;
mod session;
mod values;

pub use driver::SqliteDriver;
