//! Rust frame bridge and limited Linux PTY spike, not a full terminal.
pub mod history_search;
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

pub use jterm_protocol::file_reference;
pub use jterm_protocol::terminal as protocol;
pub use jterm_protocol::terminal_v4 as protocol_v4;
pub use protocol::{CursorShape as WireCursorShape, WireCell, WireFrame};
pub use protocol_v4::{LocalSnapshot, SnapshotFormat, SnapshotRevision};

pub fn snapshot_encoder(
    format: SnapshotFormat,
) -> impl FnOnce(&CellFrame, &str, u32) -> Result<LocalSnapshot, String> {
    move |frame, session, epoch| match format {
        SnapshotFormat::Compatibility => {
            from_engine(frame, session, epoch).map(LocalSnapshot::Compatibility)
        }
        SnapshotFormat::Channel => semantic_v4::encode(None, frame, session)
            .map(|(_, frame)| LocalSnapshot::Channel(frame)),
    }
}

/// Resolve a host-owned immutable frame into paint-only values. No ANSI reaches JS.
pub fn from_engine(frame: &CellFrame, session: &str, epoch: u32) -> Result<WireFrame, String> {
    semantic::validate_grid(frame)?;
    let revision = u32::try_from(frame.stamp().revision).map_err(|_| "revision exhausted")?;
    let layout = u32::try_from(frame.stamp().layout).map_err(|_| "layout exhausted")?;
    Ok(WireFrame {
        version: protocol::VERSION,
        session: session.into(),
        epoch,
        revision,
        layout,
        cols: frame.cols(),
        rows: frame
            .rows()
            .iter()
            .map(|row| wire_row(row, frame.cols()))
            .collect(),
        cursor_row: frame.cursor().row,
        cursor_col: frame.cursor().col,
        cursor_visible: frame.cursor().visible,
        cursor_shape: cursor_shape(frame.cursor().shape),
        pending_wrap: frame.cursor().pending_wrap,
    })
}
fn cursor_shape(shape: CursorShape) -> WireCursorShape {
    match shape {
        CursorShape::Block => WireCursorShape::Block,
        CursorShape::Underline => WireCursorShape::Underline,
        CursorShape::Beam => WireCursorShape::Beam,
    }
}

fn wire_row(row: &Row, cols: usize) -> Vec<WireCell> {
    row.cells
        .iter()
        .enumerate()
        .map(|(column, c)| {
            let mut fg = color(c.foreground, true);
            let mut bg = color(c.background, false);
            if c.flags.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            WireCell {
                text: c.character.to_string(),
                foreground: fg,
                background: bg,
                width: cell_width(c.flags, column, cols),
                style: u8::from(c.flags.bold())
                    | (u8::from(c.flags.italic()) << 1)
                    | (u8::from(c.flags.underline() != jterm_engine::UnderlineStyle::None) << 2)
                    | (u8::from(c.flags.strikethrough()) << 3)
                    | (u8::from(c.flags.dim()) << 4),
            }
        })
        .collect()
}

fn cell_width(flags: jterm_engine::StyleFlags, column: usize, cols: usize) -> u8 {
    if flags.wide_continuation() {
        0
    } else if flags.wide() && column + 1 < cols {
        2
    } else {
        1
    }
}

fn color(c: jterm_engine::Color, foreground: bool) -> String {
    use jterm_engine::Color::*;
    let index = match c {
        Black => 0,
        Red => 1,
        Green => 2,
        Yellow => 3,
        Blue => 4,
        Magenta => 5,
        Cyan => 6,
        White => 7,
        BrightBlack => 8,
        BrightRed => 9,
        BrightGreen => 10,
        BrightYellow => 11,
        BrightBlue => 12,
        BrightMagenta => 13,
        BrightCyan => 14,
        BrightWhite => 15,
        Indexed(n) => n,
        Rgb(r, g, b) => return format!("#{r:02x}{g:02x}{b:02x}"),
        Default => return if foreground { "#d9e5ef" } else { "#111820" }.into(),
    };
    let palette = [
        "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
        "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff",
    ];
    if index < 16 {
        return palette[index as usize].into();
    }
    if index >= 232 {
        let n = 8 + (index - 232) * 10;
        return format!("#{n:02x}{n:02x}{n:02x}");
    }
    let n = index - 16;
    let levels = [0u8, 95, 135, 175, 215, 255];
    format!(
        "#{:02x}{:02x}{:02x}",
        levels[(n / 36) as usize],
        levels[((n / 6) % 6) as usize],
        levels[(n % 6) as usize]
    )
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
