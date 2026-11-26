//! Rust frame bridge and limited Linux PTY spike, not a full terminal.
pub mod live;
use jterm_engine::{
    frame::{CellFrame, Cursor, Limits, Row, Screen, Stamp},
    CursorShape, TerminalGrid,
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct WireCell {
    pub text: String,
    pub foreground: String,
    pub background: String,
    pub width: u8,
    /// bold=1, italic=2, underline=4, strike=8, dim=16.
    pub style: u8,
}
#[derive(Debug, Clone, Serialize)]
pub struct WireFrame {
    pub version: u8,
    pub session: String,
    pub epoch: u32,
    pub revision: u32,
    pub layout: u32,
    pub cols: usize,
    pub rows: Vec<Vec<WireCell>>,
    pub row_wrapped: Vec<bool>,
    pub screen: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse: Option<WireMouse>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
    pub cursor_shape: &'static str,
    pub pending_wrap: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct WireMouse {
    pub tracking: &'static str,
    pub generation: u32,
}
/// Resolve a host-owned immutable frame into paint-only values. No ANSI reaches JS.
pub fn from_engine(frame: &CellFrame, session: &str, epoch: u32) -> Result<WireFrame, String> {
    let revision = u32::try_from(frame.stamp().revision).map_err(|_| "revision exhausted")?;
    let layout = u32::try_from(frame.stamp().layout).map_err(|_| "layout exhausted")?;
    Ok(WireFrame {
        version: 3,
        session: session.into(),
        epoch,
        revision,
        layout,
        cols: frame.cols(),
        rows: frame
            .rows()
            .iter()
            .map(|r| {
                r.cells
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
                            width: if c.flags.wide_continuation() {
                                0
                            } else if c.flags.wide() && column + 1 < frame.cols() {
                                2
                            } else {
                                1
                            },
                            style: u8::from(c.flags.bold())
                                | (u8::from(c.flags.italic()) << 1)
                                | (u8::from(
                                    c.flags.underline() != jterm_engine::UnderlineStyle::None,
                                ) << 2)
                                | (u8::from(c.flags.strikethrough()) << 3)
                                | (u8::from(c.flags.dim()) << 4),
                        }
                    })
                    .collect()
            })
            .collect(),
        row_wrapped: frame.rows().iter().map(|row| row.wrapped).collect(),
        screen: match frame.stamp().screen {
            Screen::Primary => "primary",
            Screen::Alternate => "alternate",
        },
        mouse: None,
        cursor_row: frame.cursor().row,
        cursor_col: frame.cursor().col,
        cursor_visible: frame.cursor().visible,
        cursor_shape: match frame.cursor().shape {
            CursorShape::Block => "block",
            CursorShape::Underline => "underline",
            CursorShape::Beam => "beam",
        },
        pending_wrap: frame.cursor().pending_wrap,
    })
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
    fn row_wrapping_and_screen_identity_reach_the_wire() {
        let mut terminal = jterm_headless_session_spike::Terminal::new(3, 4).unwrap();
        terminal.feed(b"abcde").unwrap();
        let frame = from_engine(&terminal.frame().unwrap(), "test", 1).unwrap();
        assert_eq!(frame.screen, "primary");
        assert_eq!(frame.row_wrapped, [true, false, false]);
        terminal.feed(b"\x1b[?1049h").unwrap();
        let alternate = from_engine(&terminal.frame().unwrap(), "test", 1).unwrap();
        assert_eq!(alternate.screen, "alternate");
        assert_eq!(alternate.row_wrapped, [false; 3]);
    }

    #[test]
    fn cursor_shape_and_wrap_reach_wire_without_inference() {
        let mut terminal = jterm_headless_session_spike::Terminal::new(2, 4).unwrap();
        for (sequence, expected) in [
            ("\x1b[2 q", "block"),
            ("\x1b[4 q", "underline"),
            ("\x1b[6 q", "beam"),
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
