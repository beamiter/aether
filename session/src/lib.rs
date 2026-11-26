//! Aether's session API for the shared engine and Linux PTY runtime.
pub use jterm_engine::terminal::{HistoryPage, Terminal};
#[cfg(target_os = "linux")]
pub use jterm_runtime::pty_session::Session;
#[cfg(target_os = "linux")]
pub use jterm_runtime::runtime_wake::WakeSink;
#[cfg(target_os = "linux")]
pub use jterm_runtime::terminal_profile;
