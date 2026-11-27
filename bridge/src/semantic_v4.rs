//! Convert immutable engine paint state to bounded Channel frames.

use crate::{cell_width, cursor_shape, protocol_v4 as wire, semantic::validate_grid};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jterm_engine::{
    colors::ColorState,
    frame::{CellFrame, Row, Screen},
    history_reflow::HistoryAnchor,
    retained_bytes::{RetainedBytes, RetainedBytesBudget},
    terminal::{HistoryPage, InlineImage},
    Color, HyperlinkId, UnderlineStyle,
};
use jterm_protocol::engine_projection::{paint_color, same_cell_colors};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    io::{self, Write},
    sync::{Arc, OnceLock},
};

const JSON_WORKSPACE_BYTES: usize = wire::MAX_ENCODED_FRAME_BYTES + wire::MAX_ENCODED_IMAGE_BYTES;
const PROCESS_JSON_WORKSPACE_BYTES: usize = 4 * JSON_WORKSPACE_BYTES;
const ENCODERS_BUSY: &str = "terminal channel encoders are temporarily busy";

pub(crate) struct FrameScratch {
    _reservation: RetainedBytes,
}

impl FrameScratch {
    pub(crate) fn acquire() -> Option<Self> {
        static PROCESS: OnceLock<Arc<RetainedBytesBudget>> = OnceLock::new();
        let process = PROCESS
            .get_or_init(|| Arc::new(RetainedBytesBudget::new(PROCESS_JSON_WORKSPACE_BYTES)));
        Self::reserve_from(process)
    }

    fn reserve_from(process: &Arc<RetainedBytesBudget>) -> Option<Self> {
        Some(Self {
            _reservation: process.reserve(JSON_WORKSPACE_BYTES)?,
        })
    }

    pub(crate) fn encode(
        &self,
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
}

pub(crate) fn encode(
    previous: Option<&CellFrame>,
    frame: &CellFrame,
    session: &str,
    version: u8,
) -> Result<(wire::Counter, String), String> {
    FrameScratch::acquire()
        .ok_or(ENCODERS_BUSY)?
        .encode(previous, frame, session, version)
}

fn encode_json(
    value: &impl serde::Serialize,
    images: &[wire::ImageResource],
) -> Result<String, String> {
    let limit =
        wire::MAX_ENCODED_FRAME_BYTES + images.iter().map(|image| image.rgba.len()).sum::<usize>();
    if limit > JSON_WORKSPACE_BYTES {
        return Err("terminal frame exceeds its encoding workspace".into());
    }
    let mut data = BoundedJson {
        bytes: Vec::new(),
        limit,
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
    let _scratch = FrameScratch::acquire().ok_or(ENCODERS_BUSY)?;
    let mut projection = Projection {
        images_enabled: true,
        colors: page.paint_colors(),
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
        default_background: page
            .paint_colors()
            .map(|colors| colors.effective_background().into()),
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
    let colors = frame.paint_colors();
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
        cursor_color: colors.map(|colors| colors.effective_cursor().into()),
        pending_wrap: cursor.pending_wrap,
    };
    let mut projection = Projection {
        images_enabled: matches!(version, wire::IMAGE_VERSION | wire::ROW_MOVE_VERSION),
        colors,
        ..Projection::default()
    };
    if let Some((base, changed)) = previous
        .filter(|base| same_cell_colors(base.paint_colors(), colors))
        .and_then(|base| {
            base.changed_row_indices(frame)
                .ok()
                .map(|changed| (base, changed))
        })
    {
        let (rows, moves) = if version == wire::ROW_MOVE_VERSION && changed.len() > 1 {
            split_row_moves(base, frame, changed)
        } else {
            (changed, Vec::new())
        };
        if rows.len() < frame.rows().len() {
            if projection.images_enabled
                && rows
                    .iter()
                    .any(|&index| row_images(frame, index).next().is_some())
            {
                projection.base_images = (0..base.rows().len())
                    .flat_map(|index| row_images(base, index))
                    .map(|(_, image, _)| image.id().get())
                    .collect();
            }
            let rows = rows
                .into_iter()
                .map(|index| {
                    Ok(wire::ChangedRow {
                        index: index as u16,
                        row: wire_row(frame, index, &mut projection)?,
                    })
                })
                .collect::<Result<_, String>>()?;
            let (hyperlinks, images) = projection.into_resources();
            return Ok(wire::Update::Delta {
                stamp,
                base_revision: counter(base.stamp().revision)?,
                rows,
                moves,
                hyperlinks,
                images,
            });
        }
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

fn split_row_moves(
    base: &CellFrame,
    frame: &CellFrame,
    mut rows: Vec<usize>,
) -> (Vec<usize>, Vec<wire::RowMove>) {
    let mut sources: Vec<_> = base
        .rows()
        .iter()
        .enumerate()
        .map(|(index, row)| (row.cells.as_ptr(), index))
        .collect();
    sources.sort_unstable_by_key(|&(address, _)| address);
    let mut moves: Vec<wire::RowMove> = Vec::new();
    rows.retain(|&to| {
        let address = frame.rows()[to].cells.as_ptr();
        let start = sources.partition_point(|&(candidate, _)| candidate < address);
        let from = sources[start..]
            .iter()
            .take_while(|&&(candidate, _)| candidate == address)
            .map(|&(_, index)| index)
            .find(|&from| from != to && base.shares_row_paint_with(from, frame, to));
        let Some(from) = from else {
            return true;
        };
        let (from, to) = (from as u16, to as u16);
        if let Some(last) = moves
            .last_mut()
            .filter(|last| last.from + last.count == from && last.to + last.count == to)
        {
            last.count += 1;
        } else {
            moves.push(wire::RowMove { from, to, count: 1 });
        }
        false
    });
    (rows, moves)
}

#[derive(Default)]
struct Projection<'a> {
    colors: Option<&'a ColorState>,
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
                let mut foreground = paint_color(cell.foreground, true, self.colors);
                let mut background = paint_color(cell.background, false, self.colors);
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
                    underline_color: underline(column)
                        .map(|color_value| paint_color(color_value, true, self.colors)),
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
        let required = self.bytes.len() + bytes.len();
        let capacity = self.bytes.capacity();
        if required > capacity {
            let target = required.max(capacity.saturating_mul(2)).min(self.limit);
            self.bytes
                .try_reserve_exact(target - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    // This sink accepts the whole slice or fails before appending any bytes.
    // Avoid the generic partial-write/retry loop for each JSON fragment.
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write(bytes)?;
        Ok(())
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

    #[test]
    fn process_encoding_workspace_defers_new_encoders_until_reservations_are_released() {
        let budget = Arc::new(RetainedBytesBudget::new(2 * JSON_WORKSPACE_BYTES));
        let a = FrameScratch::reserve_from(&budget).unwrap();
        let b = FrameScratch::reserve_from(&budget).unwrap();
        assert!(FrameScratch::reserve_from(&budget).is_none());
        assert_eq!(budget.usage().retained_bytes, 2 * JSON_WORKSPACE_BYTES);
        drop(a);
        let c = FrameScratch::reserve_from(&budget).unwrap();
        drop(b);
        drop(c);
        assert_eq!(budget.usage().retained_bytes, 0);
    }

    #[test]
    fn incremental_json_writes_do_not_grow_past_the_reserved_workspace() {
        let mut writer = BoundedJson {
            bytes: Vec::new(),
            limit: 19,
        };
        writer.write_all(b"1234567890").unwrap();
        let capacity = writer.bytes.capacity();
        assert!(writer.write_all(b"abcdefghij").is_err());
        assert_eq!(writer.bytes, b"1234567890");
        assert_eq!(writer.bytes.capacity(), capacity);
        writer.write_all(b"abcdefghi").unwrap();
        assert_eq!(writer.bytes, b"1234567890abcdefghi");
        assert_eq!(writer.bytes.capacity(), 19);
        assert!(writer.write_all(b"x").is_err());
        assert_eq!(writer.bytes.len(), 19);
    }

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
        // Encoding assertions own their workspace independently of parallel PTY publishers.
        let budget = Arc::new(RetainedBytesBudget::new(JSON_WORKSPACE_BYTES));
        let scratch = FrameScratch::reserve_from(&budget).unwrap();
        let mut terminal = jterm_engine::terminal::Terminal::new(5, 8).unwrap();
        terminal.enable_inline_graphics((12, 24), |_, _, _| None);
        let before = terminal.frame().unwrap();
        let command = b"\x1b[3;2H\x1b_Ga=T,i=3,s=1,v=1,c=2,r=1,f=24,C=1;/wAA\x1b\\";
        assert_eq!(terminal.feed(command).unwrap(), b"\x1b_Gi=3;OK\x1b\\");
        let displayed = terminal.frame().unwrap();
        let (_, data) = scratch
            .encode(Some(&before), &displayed, "test", wire::IMAGE_VERSION)
            .unwrap();
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
        let wire::Update::Delta {
            moves,
            rows,
            images,
            ..
        } = update(Some(&displayed), &moved, "test", wire::ROW_MOVE_VERSION).unwrap()
        else {
            panic!("the image should move from the acknowledged frame");
        };
        assert!(moves.iter().any(|part| part.to == 1 && part.from == 2));
        assert!(rows.iter().all(|row| row.row.image_slices.is_empty()));
        assert!(images.is_empty());
        let (_, data) = scratch.encode(None, &moved, "test", wire::VERSION).unwrap();
        let text_frame: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert!(text_frame.get("images").is_none());
        assert!(text_frame["rows"][1].get("image_slices").is_none());
    }

    #[test]
    fn v6_scroll_moves_preserve_full_paint_and_leave_legacy_frames_compatible() {
        let mut terminal = jterm_engine::terminal::Terminal::new(6, 12).unwrap();
        terminal.enable_hyperlinks(|_, _| true);
        terminal.enable_underline_colors();
        terminal.enable_grapheme_clusters();
        terminal
            .feed(
                concat!(
                    "A\r\n\x1b]8;;https://example.test/link\x07\x1b[4;58;5;33mBe\u{301}",
                    "\x1b[0m\x1b]8;;\x07\r\nC\r\nD\r\nE\r\nF",
                )
                .as_bytes(),
            )
            .unwrap();
        let before = terminal.frame().unwrap();
        terminal.feed(b"\x1b[S\x1b[6;1Hnew").unwrap();
        let after = terminal.frame().unwrap();
        let snapshot = |frame| update(None, frame, "test", wire::ROW_MOVE_VERSION).unwrap();
        let wire::Update::Snapshot {
            rows: base,
            hyperlinks: base_links,
            ..
        } = snapshot(&before)
        else {
            unreachable!()
        };
        let wire::Update::Snapshot {
            rows: full,
            hyperlinks: full_links,
            ..
        } = snapshot(&after)
        else {
            unreachable!()
        };
        let wire::Update::Delta {
            base_revision,
            rows,
            moves,
            hyperlinks,
            ..
        } = update(Some(&before), &after, "test", wire::ROW_MOVE_VERSION).unwrap()
        else {
            panic!("full-screen scrolling should use a row move")
        };
        assert_eq!(base_revision.get(), before.stamp().revision);
        assert_eq!(
            moves,
            [wire::RowMove {
                from: 1,
                to: 0,
                count: 5
            }]
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 5);
        assert_eq!(&base[1..], &full[..5]);
        assert_eq!(rows[0].row, full[5]);
        assert_eq!(base_links, full_links);
        assert!(hyperlinks.is_empty());

        for version in [wire::VERSION, wire::IMAGE_VERSION] {
            let legacy = update(Some(&before), &after, "test", version).unwrap();
            assert!(matches!(legacy, wire::Update::Snapshot { .. }));
            assert!(serde_json::to_value(legacy).unwrap().get("moves").is_none());
        }
    }

    #[test]
    fn snapshot_and_exact_delta_preserve_the_same_paint_data_at_u64_width() {
        // Encoding assertions own their workspace independently of parallel PTY publishers.
        let budget = Arc::new(RetainedBytesBudget::new(JSON_WORKSPACE_BYTES));
        let scratch = FrameScratch::reserve_from(&budget).unwrap();
        let revision = 9_007_199_254_740_992;
        let base = sample(revision, Screen::Primary, false);
        let next = sample(revision + 1, Screen::Primary, true);
        let (_, snapshot) = scratch.encode(None, &next, "test", wire::VERSION).unwrap();
        let (_, delta) = scratch
            .encode(Some(&base), &next, "test", wire::VERSION)
            .unwrap();
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
            moves,
        } = serde_json::from_str(&delta).unwrap()
        else {
            panic!("expected partial delta");
        };
        assert_eq!(delta_stamp, stamp);
        assert_eq!(base_revision.get(), revision);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].index, 0);
        assert_eq!(edits[0].row, rows[0]);
        assert!(moves.is_empty());
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
