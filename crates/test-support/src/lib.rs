//! Fakes and shared helpers for tests. Application tests use these instead of real databases
//! or networks.

pub mod driver_contract;
pub mod fake_driver;
pub mod fake_llm;
pub mod in_memory;

pub use fake_driver::{FakeDriver, FakeSession};
pub use fake_llm::FakeLlm;
pub use in_memory::{
    FixedClock, InMemoryConnections, InMemoryHistory, InMemorySecrets, InMemorySettings,
};

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a mutex, ignoring poisoning (fakes hold plain data, so a panic elsewhere is harmless).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
