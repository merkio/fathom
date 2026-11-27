//! Presentation layer. Screens follow State / Presenter / View; the UI thread never blocks and
//! talks to use cases only through the command/event bridge.

pub mod ai;
pub mod app;
pub mod backend;
pub mod bridge;
pub mod export;
pub mod form;
pub mod inline;
pub mod presenter;
pub mod theme;
mod views;

pub use app::{FathomApp, run};
pub use backend::Backend;
pub use bridge::{AiAsk, AiId, AiTask, Command, Event, ExportDone, SessionId, TabId};
pub use presenter::Presenter;
