//! Bounded semantic publication over a transport chosen by the host.
use crate::{cursor_shape, from_engine, protocol, wire_row};
use jterm_engine::frame::CellFrame;
pub use protocol::{Attachment, ChangedRow, Resume, SessionSelector, Update, WireDelta};

/// Produce a v3 snapshot or delta from immutable frames for a native producer.
pub fn update(
    previous: Option<&CellFrame>,
    frame: &CellFrame,
    id: &str,
    epoch: u32,
) -> Result<Update, String> {
    validate_grid(frame)?;
    if let Some(delta) = previous.and_then(|previous| previous.delta_to(frame).ok()) {
        // If the complete grid changed, the snapshot is no larger and retains
        // its independence from previously accepted state.
        if delta.rows.len() < frame.rows().len() {
            return Ok(Update::Delta {
                frame: WireDelta {
                    version: protocol::VERSION,
                    session: id.to_owned(),
                    epoch,
                    base_revision: revision(delta.base.revision)?,
                    revision: revision(delta.next.revision)?,
                    layout: u32::try_from(delta.next.layout).map_err(|_| "layout exhausted")?,
                    cursor_row: delta.cursor.row,
                    cursor_col: delta.cursor.col,
                    cursor_visible: delta.cursor.visible,
                    cursor_shape: cursor_shape(delta.cursor.shape),
                    pending_wrap: delta.cursor.pending_wrap,
                    rows: delta
                        .rows
                        .into_iter()
                        .map(|(index, row)| ChangedRow {
                            index,
                            cells: wire_row(&row, frame.cols()),
                        })
                        .collect(),
                },
            });
        }
    }
    Ok(Update::Snapshot {
        frame: from_engine(frame, id, epoch)?,
    })
}

pub(crate) fn validate_grid(frame: &CellFrame) -> Result<(), String> {
    let rows = frame.rows().len();
    let cols = frame.cols();
    if rows <= protocol::MAX_ROWS
        && cols <= protocol::MAX_COLUMNS
        && rows * cols <= protocol::MAX_CELLS
    {
        Ok(())
    } else {
        Err("terminal grid exceeds wire limit".into())
    }
}

pub(crate) fn revision(value: u64) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| "revision exhausted".into())
}

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
