use std::time::SystemTime;

/// Source of the current time, so use cases stay deterministic in tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}
