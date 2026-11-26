//! MySQL and MariaDB adapter: implements `DatabaseDriver` over `sqlx` (runtime queries only).
//!
//! One driver (kind `mysql`) serves both MySQL and MariaDB; the differences that matter (for
//! example `JSON` being an alias of `LONGTEXT` on MariaDB) are handled by the value mapping.
//!
//! Connection options: `sslmode` (`disable`, `prefer` (default), `require`, `verify-ca`,
//! `verify-full`), `sslrootcert` (CA file), `sslcert` and `sslkey` (client certificate).

mod dialect;
mod driver;
mod errors;
mod introspect;
mod session;
mod values;

pub use driver::MySqlDriver;
