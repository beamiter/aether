//! Bounded semantic publication over a transport chosen by the host.
use crate::protocol;
pub use jterm_protocol::engine_projection::update;
pub(crate) use jterm_protocol::engine_projection::validate_grid;
pub use protocol::{Attachment, ChangedRow, Resume, SessionSelector, Update, WireDelta};

pub(crate) fn session_id() -> Result<String, String> {
    use std::fmt::Write;
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| "session identity unavailable")?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        write!(&mut id, "{byte:02x}").expect("write to String");
    }
    Ok(id)
}
