use std::time::SystemTime;

/// Source of the current time, so use cases stay deterministic in tests.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

/// Wall-clock implementation backed by `std`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
