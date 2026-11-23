//! Convert immutable engine paint state to bounded Channel frames.

use crate::{cell_width, color, cursor_shape, protocol_v4 as wire, semantic::validate_grid};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jterm_engine::{
    frame::{CellFrame, Row, Screen},
    history_reflow::HistoryAnchor,
    terminal::{HistoryPage, InlineImage},
    Color, HyperlinkId, UnderlineStyle,
};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    io::{self, Write},
};

pub(crate) fn encode(
    previous: Option<&CellFrame>,
    frame: &CellFrame,
    session: &str,
    version: u8,
) -> Result<(wire::Counter, String), String> {
    let update = update(previous, frame, session, version)?;
    let images = match &update {
        wire::Update::Snapshot { images, .. } | wire::Update::Delta { images, .. } => images,
    };
    Ok((update.revision(), encode_json(&update, images)?))
}

fn encode_json(
    value: &impl serde::Serialize,
    images: &[wire::ImageResource],
) -> Result<String, String> {
    let mut data = BoundedJson {
        bytes: Vec::new(),
        limit: wire::MAX_ENCODED_FRAME_BYTES
            + images.iter().map(|image| image.rgba.len()).sum::<usize>(),
    };
    serde_json::to_writer(&mut data, value).map_err(|error| error.to_string())?;
    String::from_utf8(data.bytes).map_err(|_| "invalid frame encoding".into())
}

pub(crate) fn history_columns(page: &HistoryPage, minimum: usize) -> usize {
    (0..page.row_count)
        .filter_map(|index| page.row(index))
        .map(|row| row.cells.len())
        .fold(minimum, usize::max)
}

pub(crate) fn encode_history(
    owner: crate::semantic::Attachment,
    page: &HistoryPage,
    cols: usize,
    revision: u64,
    text: &str,
    source_at: impl Fn(usize) -> Option<HistoryAnchor>,
) -> Result<String, String> {
    if !(1..=wire::MAX_COLUMNS).contains(&cols)
        || page.row_count > wire::MAX_ROWS
        || page.total_rows > wire::MAX_PROJECTED_HISTORY_ROWS
        || page.offset + page.row_count > page.total_rows
        || text.len() > crate::protocol::MAX_HISTORY_SNAPSHOT_BYTES
        || !text.starts_with(&page.text)
    {
        return Err("invalid history projection bounds".into());
    }
    let mut projection = Projection {
        images_enabled: true,
        ..Projection::default()
    };
    let rows = page
        .copy_offsets_utf16()?
        .into_iter()
        .enumerate()
        .map(|(index, text_offsets)| {
            let row = page.row(index).ok_or("missing history row")?;
            if row.cells.len() > cols {
                return Err("history row exceeds its viewport".into());
            }
            let cells = projection.cells(
                row,
                |column| page.cluster_text(index, column),
                |id| page.hyperlink_target(id),
                |column| page.underline_color(index, column),
            )?;
            let image_slices = projection.image_slices(
                page.inline_images(index)
                    .filter(|(_, image, _)| image.raster().is_some()),
            )?;
            let source = source_at(index).ok_or("missing history source row")?;
            let row_id = source
                .row
                .checked_add(1)
                .ok_or("history source exhausted")?;
            Ok(wire::HistoryRow {
                source: wire::HistorySource {
                    row: counter(row_id)?,
                    column: source.column,
                },
                paint: wire::Row {
                    wrapped: row.wrapped,
                    cells,
                    image_slices,
                },
                text_offsets,
            })
        })
        .collect::<Result<_, String>>()?;
    let (hyperlinks, images) = projection.into_resources();
    let view = wire::HistoryView {
        owner,
        revision: counter(revision)?,
        version: wire::IMAGE_VERSION,
        cols: cols as u16,
        offset: page.offset as u32,
        total_rows: page.total_rows as u32,
        rows,
        text: text.into(),
        hyperlinks,
        images,
    };
    encode_json(&view, &view.images)
}

fn update(
    previous: Option<&CellFrame>,
    frame: &CellFrame,
    session: &str,
    version: u8,
) -> Result<wire::Update, String> {
    validate_grid(frame)?;
    let engine_stamp = frame.stamp();
    let cursor = frame.cursor();
    let stamp = wire::Stamp {
        version,
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
    let mut projection = Projection {
        images_enabled: version == wire::IMAGE_VERSION,
        ..Projection::default()
    };
    if let Some((base, delta)) = previous
        .and_then(|base| base.delta_to(frame).ok().map(|delta| (base, delta)))
        .filter(|(_, delta)| delta.rows.len() < frame.rows().len())
    {
        if projection.images_enabled {
            projection.base_images = (0..base.rows().len())
                .flat_map(|index| row_images(base, index))
                .map(|(_, image, _)| image.id().get())
                .collect();
        }
        let rows = delta
            .rows
            .iter()
            .map(|(index, _)| {
                Ok(wire::ChangedRow {
                    index: *index as u16,
                    row: wire_row(frame, *index, &mut projection)?,
                })
            })
            .collect::<Result<_, String>>()?;
        let (hyperlinks, images) = projection.into_resources();
        return Ok(wire::Update::Delta {
            stamp,
            base_revision: counter(delta.base.revision)?,
            rows,
            hyperlinks,
            images,
        });
    }
    let rows = (0..frame.rows().len())
        .map(|index| wire_row(frame, index, &mut projection))
        .collect::<Result<_, _>>()?;
    let (hyperlinks, images) = projection.into_resources();
    Ok(wire::Update::Snapshot {
        stamp,
        cols: frame.cols() as u16,
        rows,
        hyperlinks,
        images,
    })
}

#[derive(Default)]
struct Projection<'a> {
    text_bytes: usize,
    hyperlink_bytes: usize,
    hyperlinks: BTreeMap<u16, &'a str>,
    images_enabled: bool,
    image_bytes: usize,
    base_images: BTreeSet<u64>,
    images: BTreeMap<u64, wire::ImageResource>,
}

impl<'a> Projection<'a> {
    fn into_resources(self) -> (Vec<wire::Hyperlink>, Vec<wire::ImageResource>) {
        let hyperlinks = self
            .hyperlinks
            .into_iter()
            .map(|(id, target)| wire::Hyperlink {
                id,
                target: target.into(),
            })
            .collect();
        (hyperlinks, self.images.into_values().collect())
    }

    fn image_slices(
        &mut self,
        images: impl Iterator<Item = (usize, &'a InlineImage, u16)>,
    ) -> Result<Vec<wire::ImageSlice>, String> {
        if !self.images_enabled {
            return Ok(Vec::new());
        }
        images
            .map(|(column, image, slice)| {
                let key = image.id().get();
                let id = counter(key)?;
                if !self.base_images.contains(&key) && !self.images.contains_key(&key) {
                    let raster = image.raster().ok_or("missing inline raster")?;
                    self.image_bytes += raster.rgba().len();
                    if self.images.len() >= wire::MAX_IMAGES
                        || self.image_bytes > wire::MAX_IMAGE_BYTES
                        || raster.width() > wire::MAX_IMAGE_DIMENSION
                        || raster.height() > wire::MAX_IMAGE_DIMENSION
                    {
                        return Err("terminal frame exceeds image resource limit".into());
                    }
                    self.images.insert(
                        key,
                        wire::ImageResource {
                            id,
                            width: raster.width(),
                            height: raster.height(),
                            columns: image.columns(),
                            rows: image.rows(),
                            rgba: STANDARD.encode(raster.rgba()),
                        },
                    );
                }
                Ok(wire::ImageSlice {
                    column: column as u16,
                    id,
                    slice,
                })
            })
            .collect()
    }
    fn cells(
        &mut self,
        row: &Row,
        cluster: impl Fn(usize) -> Option<&'a str>,
        hyperlink: impl Fn(HyperlinkId) -> Option<&'a str>,
        underline: impl Fn(usize) -> Option<Color>,
    ) -> Result<Vec<wire::Cell>, String> {
        row.cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                let flags = cell.flags;
                let width = cell_width(flags, column, row.cells.len());
                let mut scalar = [0; 4];
                let text = if width == 0 {
                    " "
                } else {
                    cluster(column).unwrap_or_else(|| cell.character.encode_utf8(&mut scalar))
                };
                self.text_bytes += text.len();
                if text.len() > wire::MAX_CLUSTER_BYTES || self.text_bytes > wire::MAX_TEXT_BYTES {
                    return Err("terminal frame exceeds text limit".into());
                }
                let hyperlink_id = if cell.hyperlink_id.is_none() {
                    None
                } else {
                    let id = cell.hyperlink_id.as_raw();
                    if usize::from(id) > wire::MAX_HYPERLINKS {
                        return Err("terminal hyperlink ID exceeds limit".into());
                    }
                    if let Entry::Vacant(entry) = self.hyperlinks.entry(id) {
                        let target = hyperlink(cell.hyperlink_id)
                            .ok_or("terminal frame has an unresolved hyperlink")?;
                        self.hyperlink_bytes += target.len();
                        if target.len() > wire::MAX_HYPERLINK_TARGET_BYTES
                            || self.hyperlink_bytes > wire::MAX_HYPERLINK_BYTES
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
                    underline_color: underline(column).map(|color_value| color(color_value, true)),
                    hyperlink_id,
                })
            })
            .collect()
    }
}

fn row_images(frame: &CellFrame, index: usize) -> impl Iterator<Item = (usize, &InlineImage, u16)> {
    frame.inline_images(index).filter(|(column, image, _)| {
        image.raster().is_some() && column + usize::from(image.columns()) <= frame.cols()
    })
}

fn wire_row<'a>(
    frame: &'a CellFrame,
    index: usize,
    projection: &mut Projection<'a>,
) -> Result<wire::Row, String> {
    let row = &frame.rows()[index];
    Ok(wire::Row {
        wrapped: row.wrapped,
        cells: projection.cells(
            row,
            |column| frame.cluster_text(index, column),
            |id| frame.hyperlink_target(id),
            |column| frame.underline_color(index, column),
        )?,
        image_slices: projection.image_slices(row_images(frame, index))?,
    })
}

fn counter(value: u64) -> Result<wire::Counter, String> {
    wire::Counter::new(value).ok_or_else(|| "invalid terminal frame counter".into())
}

struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(io::Error::other("terminal frame exceeds JSON byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
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
    fn v5_deltas_reuse_the_visible_resource_and_v4_omits_rasters() {
        let mut terminal = jterm_engine::terminal::Terminal::new(5, 8).unwrap();
        terminal.enable_inline_graphics((12, 24), |_, _, _| None);
        let before = terminal.frame().unwrap();
        let command = b"\x1b[3;2H\x1b_Ga=T,i=3,s=1,v=1,c=2,r=1,f=24,C=1;/wAA\x1b\\";
        assert_eq!(terminal.feed(command).unwrap(), b"\x1b_Gi=3;OK\x1b\\");
        let displayed = terminal.frame().unwrap();
        let (_, data) = encode(Some(&before), &displayed, "test", wire::IMAGE_VERSION).unwrap();
        let wire::Update::Delta {
            stamp,
            rows,
            images,
            ..
        } = serde_json::from_str(&data).unwrap()
        else {
            panic!("new image must arrive in its changed row");
        };
        assert_eq!(stamp.version, wire::IMAGE_VERSION);
        assert_eq!(images.len(), 1);
        assert_eq!(rows[0].row.image_slices[0].id, images[0].id);
        assert_eq!(images[0].rgba, "/wAA/w==");
        terminal.feed(b"\x1b[1S").unwrap();
        let moved = terminal.frame().unwrap();
        let wire::Update::Delta { rows, images, .. } =
            update(Some(&displayed), &moved, "test", wire::IMAGE_VERSION).unwrap()
        else {
            panic!("scroll must move the image in the delta");
        };
        assert!(images.is_empty());
        assert!(rows
            .iter()
            .any(|row| row.index == 1 && row.row.image_slices.len() == 1));
        let (_, data) = encode(None, &moved, "test", wire::VERSION).unwrap();
        let text_frame: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(text_frame.get("images").is_none());
        assert!(text_frame["rows"][1].get("image_slices").is_none());
    }

    #[test]
    fn snapshot_and_exact_delta_preserve_the_same_paint_data_at_u64_width() {
        let revision = 9_007_199_254_740_992;
        let base = sample(revision, Screen::Primary, false);
        let next = sample(revision + 1, Screen::Primary, true);
        let (_, snapshot) = encode(None, &next, "test", wire::VERSION).unwrap();
        let (_, delta) = encode(Some(&base), &next, "test", wire::VERSION).unwrap();
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
            images,
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
        assert!(images.is_empty());
        let alternate = sample(revision + 2, Screen::Alternate, false);
        assert!(matches!(
            update(Some(&next), &alternate, "test", wire::VERSION).unwrap(),
            wire::Update::Snapshot { stamp, .. } if stamp.screen == wire::Screen::Alternate
        ));
    }

    #[test]
    fn color_only_engine_change_survives_a_delta_and_explicit_reset() {
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 4).unwrap();
        terminal.enable_underline_colors();
        terminal.feed(b"\x1b[4:2;58:2::1:2:3mX").unwrap();
        let base = terminal.frame().unwrap();
        let wire::Update::Snapshot { rows, .. } =
            update(None, &base, "test", wire::VERSION).unwrap()
        else {
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
        let wire::Update::Delta { rows, .. } =
            update(Some(&base), &next, "test", wire::VERSION).unwrap()
        else {
            panic!("expected color-only delta");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].row.cells[0].underline_color.as_deref(),
            Some("#00d787")
        );
        terminal.feed(b"\r\x1b[59mX").unwrap();
        let reset = terminal.frame().unwrap();
        let wire::Update::Delta { rows, .. } =
            update(Some(&next), &reset, "test", wire::VERSION).unwrap()
        else {
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
        } = update(None, &base, "test", wire::VERSION).unwrap()
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
        } = update(Some(&base), &next, "test", wire::VERSION).unwrap()
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
        let wire::Update::Delta { rows, .. } =
            update(Some(&base), &composed, "test", wire::VERSION).unwrap()
        else {
            panic!("expected cluster-only delta");
        };
        assert_eq!(rows[0].row.cells[0].text, "e\u{0301}");
        for chunk in ["👩", "\u{200d}", "💻"] {
            terminal.feed(chunk.as_bytes()).unwrap();
        }
        let frame = terminal.frame().unwrap();
        let wire::Update::Snapshot { rows, .. } =
            update(None, &frame, "test", wire::VERSION).unwrap()
        else {
            panic!("expected full cluster frame");
        };
        assert_eq!(rows[0].cells[1].text, "👩‍💻");
        assert_eq!(rows[0].cells[1].width, 2);
        assert_eq!(rows[0].cells[2].width, 0);
    }
}
