//! PostgreSQL adapter: implements `DatabaseDriver` over `sqlx` (runtime queries only).

mod dialect;
mod driver;
mod errors;
mod introspect;
mod session;
mod values;

pub use driver::PostgresDriver;
