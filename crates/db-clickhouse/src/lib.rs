//! ClickHouse adapter: implements `DatabaseDriver` with the official `clickhouse` client over the
//! HTTP interface.
//!
//! Connection mapping: `host` and `port` (default 8123, or 8443 with TLS), `database` (optional,
//! default `default`), `username` (the server's default user is `default`) and the password from
//! the secret store. Profile options: `sslmode=require|verify-ca|verify-full` or `secure=true`
//! switch to HTTPS (the certificate is always verified against the platform's root store).
//!
//! ClickHouse differences worth knowing: there are no foreign keys, `UPDATE`/`DELETE` are
//! asynchronous mutations that report no row count, and a statement is identified by a query id
//! so it can be killed server-side on cancel or timeout.

mod driver;
mod error;
mod introspect;
mod session;
mod values;

pub use driver::ClickHouseDriver;
