//! Rust frame bridge and limited Linux PTY spike, not a full terminal.
pub mod history_search;
mod inline_png;
pub mod live;
#[cfg(target_os = "linux")]
pub mod remote_files;
pub mod semantic;
mod semantic_v4;
#[cfg(target_os = "linux")]
pub mod terminal_transport;
use jterm_engine::{
    frame::{CellFrame, Cursor, Limits, Row, Screen, Stamp},
    CursorShape, TerminalGrid,
};
use std::sync::Arc;

#[cfg(test)]
use jterm_protocol::engine_projection::color;
pub use jterm_protocol::engine_projection::from_engine;
use jterm_protocol::engine_projection::{cell_width, cursor_shape};
pub use jterm_protocol::file_reference;
pub use jterm_protocol::terminal as protocol;
pub use jterm_protocol::terminal_v4 as protocol_v4;
pub use protocol::{CursorShape as WireCursorShape, WireCell, WireFrame};
pub use protocol_v4::{LocalSnapshot, SnapshotFormat, SnapshotRevision};

/// SSH frame version with a byte-packed representation for dense rows.
pub const SSH_DENSE_FRAME_VERSION: u8 = 9;

pub fn snapshot_encoder(
    format: SnapshotFormat,
) -> impl FnOnce(&CellFrame, &str, u32) -> Result<LocalSnapshot, String> {
    move |frame, session, epoch| match format {
        SnapshotFormat::Compatibility => {
            from_engine(frame, session, epoch).map(LocalSnapshot::Compatibility)
        }
        SnapshotFormat::Channel
        | SnapshotFormat::ChannelImages
        | SnapshotFormat::ChannelRowMoves
        | SnapshotFormat::ChannelPacked
        | SnapshotFormat::ChannelRendition => {
            let version = format.channel_version().expect("Channel format");
            semantic_v4::encode(None, frame, session, version)
                .map(|(_, data)| LocalSnapshot::Channel(data))
        }
    }
}

/// A single fixed demo session. Real session identity must come from host authority.
pub struct DemoBridge {
    frame: Option<CellFrame>,
    acknowledged: bool,
    revision: u32,
}
impl Default for DemoBridge {
    fn default() -> Self {
        Self {
            frame: None,
            acknowledged: true,
            revision: 0,
        }
    }
}
impl DemoBridge {
    pub fn next_frame(&mut self) -> Result<WireFrame, String> {
        if !self.acknowledged {
            return Err("one frame is already in flight".into());
        }
        let revision = self.revision.checked_add(1).ok_or("revision exhausted")?;
        let mut grid = TerminalGrid::new(8, 64);
        let lines = [
            "Tauri 2 + jterm_engine frame bridge",
            "Rust owns the grid. Canvas only paints.",
            "This is a frame demo, NOT an interactive shell.",
        ];
        for (r, text) in lines.iter().enumerate() {
            for (c, ch) in text.chars().enumerate() {
                grid.get_mut(r, c).character = ch;
            }
        }
        let line = format!("Verified frame {revision}; acknowledge before advancing.");
        for (c, ch) in line.chars().enumerate() {
            grid.get_mut(5, c).character = ch;
        }
        let rows: Vec<Row> = (0..grid.rows())
            .map(|r| Row {
                cells: Arc::from(grid[r].to_vec()),
                wrapped: false,
            })
            .collect();
        let frame = CellFrame::new(
            Stamp {
                epoch: 1,
                revision: revision.into(),
                layout: 1,
                screen: Screen::Primary,
            },
            64,
            rows.into(),
            Cursor {
                row: 6,
                col: 0,
                pending_wrap: false,
                visible: true,
                shape: CursorShape::Block,
            },
            Limits {
                max_rows: 128,
                max_cols: 256,
                max_cells: 32768,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        let wire = from_engine(&frame, "frame-demo", 1)?;
        self.frame = Some(frame);
        self.revision = revision;
        self.acknowledged = false;
        Ok(wire)
    }
    pub fn acknowledge(&mut self, session: &str, epoch: u32, revision: u32) -> Result<(), String> {
        if session != "frame-demo" || epoch != 1 || self.acknowledged || revision != self.revision {
            return Err("stale or foreign acknowledgement".into());
        }
        self.acknowledged = true;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_shape_and_wrap_reach_wire_without_inference() {
        let mut terminal = jterm_headless_session_spike::Terminal::new(2, 4).unwrap();
        for (sequence, expected) in [
            ("\x1b[2 q", WireCursorShape::Block),
            ("\x1b[4 q", WireCursorShape::Underline),
            ("\x1b[6 q", WireCursorShape::Beam),
        ] {
            terminal.feed(sequence.as_bytes()).unwrap();
            assert_eq!(
                from_engine(&terminal.frame().unwrap(), "test", 1)
                    .unwrap()
                    .cursor_shape,
                expected
            );
        }
        terminal.feed(b"abcd").unwrap();
        let wire = from_engine(&terminal.frame().unwrap(), "test", 1).unwrap();
        assert_eq!(wire.cursor_col, 3);
        assert!(wire.pending_wrap);
    }
    #[test]
    fn one_column_wide_scalar_is_preserved_with_clipped_paint_span() {
        let mut terminal = jterm_headless_session_spike::Terminal::new(2, 1).unwrap();
        terminal.feed("中".as_bytes()).unwrap();
        let wire = from_engine(&terminal.frame().unwrap(), "test", 1).unwrap();
        let cell = wire
            .rows
            .iter()
            .flat_map(|r| r.iter())
            .find(|c| c.text == "中")
            .unwrap();
        assert_eq!(cell.width, 1);
    }
    #[test]
    fn palette_cube_grayscale_and_truecolor_are_exact() {
        use jterm_engine::Color;
        assert_eq!(color(Color::Rgb(18, 52, 86), true), "#123456");
        assert_eq!(color(Color::Indexed(16), true), "#000000");
        assert_eq!(color(Color::Indexed(21), true), "#0000ff");
        assert_eq!(color(Color::Indexed(196), true), "#ff0000");
        assert_eq!(color(Color::Indexed(231), true), "#ffffff");
        assert_eq!(color(Color::Indexed(232), true), "#080808");
        assert_eq!(color(Color::Indexed(255), true), "#eeeeee");
        for index in 0..=255 {
            let value = color(Color::Indexed(index), true);
            assert_eq!(value.len(), 7);
            assert!(value[1..].bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
    #[test]
    fn real_engine_frame_is_serializable() {
        let f = DemoBridge::default().next_frame().unwrap();
        assert_eq!(f.rows.len(), 8);
        assert!(f.rows.iter().all(|r| r.len() == 64));
        assert!(serde_json::to_vec(&f).unwrap().len() < 65536);
    }
    #[test]
    fn exact_ack_limits_flight() {
        let mut b = DemoBridge::default();
        b.next_frame().unwrap();
        assert!(b.next_frame().is_err());
        assert!(b.acknowledge("other", 1, 1).is_err());
        assert!(b.acknowledge("frame-demo", 2, 1).is_err());
        assert!(b.acknowledge("frame-demo", 1, 2).is_err());
        b.acknowledge("frame-demo", 1, 1).unwrap();
        assert!(b.acknowledge("frame-demo", 1, 1).is_err());
        assert_eq!(b.next_frame().unwrap().revision, 2);
    }
}
