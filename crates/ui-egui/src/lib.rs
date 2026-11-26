//! Presentation layer. Screens follow State / Presenter / View; the UI thread never blocks and
//! talks to use cases only through the command/event bridge.

pub mod app;
pub mod backend;
pub mod bridge;
pub mod form;
pub mod presenter;
mod views;

pub use app::{DbieverApp, run};
pub use backend::Backend;
pub use bridge::{Command, Event, SessionId, TabId};
pub use presenter::Presenter;
