//! Convert the current immutable engine paint state to a bounded v4 frame.

use crate::{cell_width, color, cursor_shape, protocol_v4 as wire, semantic::validate_grid};
use jterm_engine::{
    frame::{CellFrame, Screen},
    UnderlineStyle,
};
use std::{
    collections::{btree_map::Entry, BTreeMap},
    io::{self, Write},
};

pub(crate) fn encode(
    previous: Option<&CellFrame>,
    frame: &CellFrame,
    session: &str,
) -> Result<(wire::Counter, String), String> {
    let update = update(previous, frame, session)?;
    let mut data = BoundedJson(Vec::new());
    serde_json::to_writer(&mut data, &update).map_err(|e| e.to_string())?;
    Ok((
        update.revision(),
        String::from_utf8(data.0).map_err(|_| "invalid frame encoding")?,
    ))
}

fn update(
    previous: Option<&CellFrame>,
    frame: &CellFrame,
    session: &str,
) -> Result<wire::Update, String> {
    validate_grid(frame)?;
    let engine_stamp = frame.stamp();
    let cursor = frame.cursor();
    let stamp = wire::Stamp {
        version: wire::VERSION,
        session: session.into(),
        epoch: counter(engine_stamp.epoch)?,
        revision: counter(engine_stamp.revision)?,
        layout: counter(engine_stamp.layout)?,
        resource_generation: counter(engine_stamp.epoch)?,
        screen: match engine_stamp.screen {
            Screen::Primary => wire::Screen::Primary,
            Screen::Alternate => wire::Screen::Alternate,
        },
        cursor_row: cursor.row as u16,
        cursor_col: cursor.col as u16,
        cursor_visible: cursor.visible,
        cursor_shape: cursor_shape(cursor.shape),
        pending_wrap: cursor.pending_wrap,
    };
    let mut projection = Projection::default();
    if let Some(delta) = previous
        .and_then(|previous| previous.delta_to(frame).ok())
        .filter(|delta| delta.rows.len() < frame.rows().len())
    {
        return Ok(wire::Update::Delta {
            stamp,
            base_revision: counter(delta.base.revision)?,
            rows: delta
                .rows
                .iter()
                .map(|(index, _)| {
                    Ok(wire::ChangedRow {
                        index: *index as u16,
                        row: wire_row(frame, *index, &mut projection)?,
                    })
                })
                .collect::<Result<_, String>>()?,
            hyperlinks: projection.into_hyperlinks(),
        });
    }
    Ok(wire::Update::Snapshot {
        stamp,
        cols: frame.cols() as u16,
        rows: (0..frame.rows().len())
            .map(|index| wire_row(frame, index, &mut projection))
            .collect::<Result<_, _>>()?,
        hyperlinks: projection.into_hyperlinks(),
    })
}

#[derive(Default)]
struct Projection<'a> {
    text_bytes: usize,
    hyperlink_bytes: usize,
    hyperlinks: BTreeMap<u16, &'a str>,
}

impl Projection<'_> {
    fn into_hyperlinks(self) -> Vec<wire::Hyperlink> {
        self.hyperlinks
            .into_iter()
            .map(|(id, target)| wire::Hyperlink {
                id,
                target: target.into(),
            })
            .collect()
    }
}

fn wire_row<'a>(
    frame: &'a CellFrame,
    index: usize,
    projection: &mut Projection<'a>,
) -> Result<wire::Row, String> {
    let row = &frame.rows()[index];
    let cells = row
        .cells
        .iter()
        .enumerate()
        .map(|(column, cell)| {
            let flags = cell.flags;
            let width = cell_width(flags, column, row.cells.len());
            let mut scalar = [0; 4];
            let text = if width == 0 {
                " "
            } else {
                frame
                    .cluster_text(index, column)
                    .unwrap_or_else(|| cell.character.encode_utf8(&mut scalar))
            };
            projection.text_bytes += text.len();
            if text.len() > wire::MAX_CLUSTER_BYTES || projection.text_bytes > wire::MAX_TEXT_BYTES
            {
                return Err("terminal frame exceeds text limit".into());
            }
            let hyperlink_id = if cell.hyperlink_id.is_none() {
                None
            } else {
                let id = cell.hyperlink_id.as_raw();
                if usize::from(id) > wire::MAX_HYPERLINKS {
                    return Err("terminal hyperlink ID exceeds limit".into());
                }
                if let Entry::Vacant(entry) = projection.hyperlinks.entry(id) {
                    let target = frame
                        .hyperlink_target(cell.hyperlink_id)
                        .ok_or("terminal frame has an unresolved hyperlink")?;
                    projection.hyperlink_bytes += target.len();
                    if target.len() > wire::MAX_HYPERLINK_TARGET_BYTES
                        || projection.hyperlink_bytes > wire::MAX_HYPERLINK_BYTES
                    {
                        return Err("terminal frame exceeds hyperlink byte limit".into());
                    }
                    entry.insert(target);
                }
                Some(id)
            };
            let mut foreground = color(cell.foreground, true);
            let mut background = color(cell.background, false);
            if flags.inverse() {
                std::mem::swap(&mut foreground, &mut background);
            }
            Ok(wire::Cell {
                text: text.to_string(),
                foreground,
                background,
                width,
                style: u8::from(flags.bold())
                    | (u8::from(flags.italic()) << 1)
                    | (u8::from(flags.strikethrough()) << 3)
                    | (u8::from(flags.dim()) << 4)
                    | (u8::from(flags.blink()) << 5)
                    | (u8::from(flags.protected()) << 6),
                underline: match flags.underline() {
                    UnderlineStyle::None => None,
                    UnderlineStyle::Single => Some(wire::Underline::Single),
                    UnderlineStyle::Double => Some(wire::Underline::Double),
                    UnderlineStyle::Curly => Some(wire::Underline::Curly),
                    UnderlineStyle::Dotted => Some(wire::Underline::Dotted),
                    UnderlineStyle::Dashed => Some(wire::Underline::Dashed),
                },
                underline_color: frame
                    .underline_color(index, column)
                    .map(|color_value| color(color_value, true)),
                hyperlink_id,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(wire::Row {
        wrapped: row.wrapped,
        cells,
    })
}

fn counter(value: u64) -> Result<wire::Counter, String> {
    wire::Counter::new(value).ok_or_else(|| "invalid terminal frame counter".into())
}

struct BoundedJson(Vec<u8>);

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > wire::MAX_ENCODED_FRAME_BYTES - self.0.len() {
            return Err(io::Error::other("terminal frame exceeds JSON byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_engine::{
        frame::{Cursor, Limits, Row, Stamp},
        CursorShape, TerminalCell,
    };
    use std::sync::Arc;

    fn sample(revision: u64, screen: Screen, wrapped: bool) -> CellFrame {
        let mut cell = TerminalCell {
            character: '中',
            ..TerminalCell::default()
        };
        cell.flags.set_wide(true);
        cell.flags.set_underline(UnderlineStyle::Curly);
        cell.flags.set_blink(true);
        let mut continuation = TerminalCell::default();
        continuation.flags.set_wide_continuation(true);
        CellFrame::new(
            Stamp {
                epoch: 7,
                revision,
                layout: u64::MAX,
                screen,
            },
            2,
            Arc::from([
                Row {
                    cells: Arc::from([cell, continuation]),
                    wrapped,
                },
                Row {
                    cells: Arc::from([TerminalCell::default(); 2]),
                    wrapped: false,
                },
            ]),
            Cursor {
                row: 0,
                col: 1,
                pending_wrap: true,
                visible: true,
                shape: CursorShape::Beam,
            },
            Limits {
                max_rows: 2,
                max_cols: 2,
                max_cells: 4,
            },
        )
        .unwrap()
    }

    #[test]
    fn snapshot_and_exact_delta_preserve_the_same_paint_data_at_u64_width() {
        let revision = 9_007_199_254_740_992;
        let base = sample(revision, Screen::Primary, false);
        let next = sample(revision + 1, Screen::Primary, true);
        let (_, snapshot) = encode(None, &next, "test").unwrap();
        let (_, delta) = encode(Some(&base), &next, "test").unwrap();
        let wire::Update::Snapshot { stamp, rows, .. } = serde_json::from_str(&snapshot).unwrap()
        else {
            panic!("expected snapshot");
        };
        assert_eq!(stamp.revision.get(), revision + 1);
        assert_eq!(stamp.layout.get(), u64::MAX);
        assert_eq!(rows[0].cells[0].underline, Some(wire::Underline::Curly));
        assert_eq!(rows[0].cells[0].underline_color, None);
        assert_eq!(rows[0].cells[0].style, 32);
        assert_eq!(rows[0].cells[0].hyperlink_id, None);
        assert_eq!(rows[0].cells[0].width, 2);
        assert_eq!(rows[0].cells[1].width, 0);
        assert!(rows[0].wrapped);
        let wire::Update::Delta {
            stamp: delta_stamp,
            base_revision,
            rows: edits,
            hyperlinks,
        } = serde_json::from_str(&delta).unwrap()
        else {
            panic!("expected partial delta");
        };
        assert_eq!(delta_stamp, stamp);
        assert_eq!(base_revision.get(), revision);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].index, 0);
        assert_eq!(edits[0].row, rows[0]);
        assert!(hyperlinks.is_empty());
        let alternate = sample(revision + 2, Screen::Alternate, false);
        assert!(matches!(
            update(Some(&next), &alternate, "test").unwrap(),
            wire::Update::Snapshot { stamp, .. } if stamp.screen == wire::Screen::Alternate
        ));
    }

    #[test]
    fn color_only_engine_change_survives_a_delta_and_explicit_reset() {
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 4).unwrap();
        terminal.enable_underline_colors();
        terminal.feed(b"\x1b[4:2;58:2::1:2:3mX").unwrap();
        let base = terminal.frame().unwrap();
        let wire::Update::Snapshot { rows, .. } = update(None, &base, "test").unwrap() else {
            panic!("expected color snapshot");
        };
        assert_eq!(rows[0].cells[0].underline_color.as_deref(), Some("#010203"));
        terminal.feed(b"\r\x1b[58:5:42mX").unwrap();
        let next = terminal.frame().unwrap();
        assert_eq!(
            base.rows(),
            next.rows(),
            "only the color sidecar should differ"
        );
        let wire::Update::Delta { rows, .. } = update(Some(&base), &next, "test").unwrap() else {
            panic!("expected color-only delta");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].row.cells[0].underline_color.as_deref(),
            Some("#00d787")
        );
        terminal.feed(b"\r\x1b[59mX").unwrap();
        let reset = terminal.frame().unwrap();
        let wire::Update::Delta { rows, .. } = update(Some(&next), &reset, "test").unwrap() else {
            panic!("expected color reset delta");
        };
        assert_eq!(rows[0].row.cells[0].underline_color, None);
        assert_eq!(
            rows[0].row.cells[0].underline,
            Some(wire::Underline::Double)
        );
    }

    #[test]
    fn only_links_referenced_by_emitted_rows_enter_the_resource_payload() {
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 8).unwrap();
        terminal.enable_hyperlinks(|_, target| target.starts_with("https://"));
        terminal.feed(b"\x1b]8;;https://first.test\x07A\x1b]8;;\x07\r\n\x1b]8;;https://second.test\x07B\x1b]8;;\x07").unwrap();
        let base = terminal.frame().unwrap();
        let wire::Update::Snapshot {
            stamp,
            rows,
            hyperlinks,
            ..
        } = update(None, &base, "test").unwrap()
        else {
            panic!("expected link snapshot");
        };
        assert_eq!(stamp.resource_generation, stamp.epoch);
        assert_eq!(hyperlinks.len(), 2);
        assert_eq!(rows[0].cells[0].hyperlink_id, Some(hyperlinks[0].id));
        assert_eq!(hyperlinks[0].target, "https://first.test");
        assert_eq!(hyperlinks[1].target, "https://second.test");
        terminal
            .feed(b"\x1b[H\x1b]8;;https://second.test\x07A\x1b]8;;\x07")
            .unwrap();
        let next = terminal.frame().unwrap();
        let wire::Update::Delta {
            rows, hyperlinks, ..
        } = update(Some(&base), &next, "test").unwrap()
        else {
            panic!("expected link delta");
        };
        assert_eq!(hyperlinks.len(), 1);
        assert_eq!(hyperlinks[0].target, "https://second.test");
        assert_eq!(rows[0].row.cells[0].hyperlink_id, Some(hyperlinks[0].id));
    }

    #[test]
    fn combining_only_delta_and_a_wide_joined_cluster_survive_encoding() {
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 8).unwrap();
        terminal.enable_grapheme_clusters();
        terminal.feed(b"e").unwrap();
        let base = terminal.frame().unwrap();
        terminal.feed("\u{0301}".as_bytes()).unwrap();
        let composed = terminal.frame().unwrap();
        assert_eq!(base.rows(), composed.rows());
        let wire::Update::Delta { rows, .. } = update(Some(&base), &composed, "test").unwrap()
        else {
            panic!("expected cluster-only delta");
        };
        assert_eq!(rows[0].row.cells[0].text, "e\u{0301}");
        for chunk in ["👩", "\u{200d}", "💻"] {
            terminal.feed(chunk.as_bytes()).unwrap();
        }
        let frame = terminal.frame().unwrap();
        let wire::Update::Snapshot { rows, .. } = update(None, &frame, "test").unwrap() else {
            panic!("expected full cluster frame");
        };
        assert_eq!(rows[0].cells[1].text, "👩‍💻");
        assert_eq!(rows[0].cells[1].width, 2);
        assert_eq!(rows[0].cells[2].width, 0);
    }
}
