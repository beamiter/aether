//! Bounded semantic publication over a transport chosen by the host.
use crate::{cursor_shape, from_engine, wire_row, WireCell, WireFrame};
use jterm_engine::frame::CellFrame;
use serde::{Deserialize, Serialize};

/// The session ID names a PTY for its lifetime. An attachment names one
/// receiver's current snapshot and invalidates its previous in-flight work.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub session: String,
    pub epoch: u32,
    pub generation: u32,
}

#[derive(Debug, Serialize)]
pub struct Resume {
    pub owner: Attachment,
    pub frame: WireFrame,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Update {
    Snapshot { frame: WireFrame },
    Delta { frame: WireDelta },
}

#[derive(Debug, Serialize)]
pub struct WireDelta {
    pub version: u8,
    pub session: String,
    pub epoch: u32,
    pub base_revision: u32,
    pub revision: u32,
    pub layout: u32,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub cursor_shape: &'static str,
    pub pending_wrap: bool,
    pub rows: Vec<ChangedRow>,
}

#[derive(Debug, Serialize)]
pub struct ChangedRow {
    pub index: usize,
    pub cells: Vec<WireCell>,
}

pub(crate) fn update(
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
                    version: 3,
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
    if rows <= 128 && cols <= 256 && rows * cols <= 32_768 {
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
