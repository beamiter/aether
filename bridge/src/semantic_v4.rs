//! Convert immutable engine paint state to bounded Channel frames.

use crate::{
    cell_width, cursor_shape, protocol_v4 as wire, semantic::validate_grid, DENSE_FRAME_VERSION,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use jterm_engine::{
    colors::ColorState,
    frame::{CellFrame, Row, Screen},
    grid::UnderlineColorRun,
    history_reflow::HistoryAnchor,
    retained_bytes::{RetainedBytes, RetainedBytesBudget},
    terminal::{HistoryPage, InlineImage},
    HyperlinkId, UnderlineStyle,
};
use jterm_protocol::{
    engine_projection::{paint_rgb, same_cell_colors},
    terminal::RgbColor,
};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    io::{self, Write},
    sync::{Arc, OnceLock},
};

const JSON_WORKSPACE_BYTES: usize = wire::MAX_ENCODED_FRAME_BYTES + wire::MAX_ENCODED_IMAGE_BYTES;
const PROCESS_JSON_WORKSPACE_BYTES: usize = 4 * JSON_WORKSPACE_BYTES;
const ENCODERS_BUSY: &str = "terminal channel encoders are temporarily busy";

/// Parts the native Channel can carry outside a frame's JSON.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinaryChannelParts {
    pub cells: bool,
    pub rasters: bool,
}

/// A bounded JSON frame and its separately carried cell and image bytes.
#[derive(Debug)]
pub struct EncodedChannelFrame {
    json: String,
    cells: Vec<u8>,
    rgba: Vec<u8>,
}

impl EncodedChannelFrame {
    pub fn as_json(&self) -> &str {
        &self.json
    }

    pub fn into_json(self) -> String {
        self.json
    }

    pub fn image_bytes(&self) -> usize {
        self.rgba.len()
    }

    pub fn cell_bytes(&self) -> usize {
        self.cells.len()
    }

    pub fn into_binary_parts(self) -> (Vec<u8>, Vec<u8>) {
        (self.cells, self.rgba)
    }

    pub fn into_image_bytes(self) -> Vec<u8> {
        self.rgba
    }
}

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
        self.encode_frame(
            previous,
            frame,
            session,
            version,
            BinaryChannelParts::default(),
        )
        .map(|(revision, frame)| (revision, frame.into_json()))
    }

    pub(crate) fn encode_frame(
        &self,
        previous: Option<&CellFrame>,
        frame: &CellFrame,
        session: &str,
        version: u8,
        binary: BinaryChannelParts,
    ) -> Result<(wire::Counter, EncodedChannelFrame), String> {
        if version == DENSE_FRAME_VERSION {
            let mut cells = Vec::new();
            let projected =
                project_update(previous, frame, session, version, binary.rasters, |row| {
                    pack_dense_row(row, binary.cells.then_some(&mut cells))
                })?;
            encode_update(projected, cells)
        } else if wire::supports_packed_rows(version) {
            encode_update(
                project_update(previous, frame, session, version, binary.rasters, pack_row)?,
                Vec::new(),
            )
        } else {
            encode_update(
                project_update(
                    previous,
                    frame,
                    session,
                    version,
                    binary.rasters,
                    std::convert::identity,
                )?,
                Vec::new(),
            )
        }
    }
}

fn encode_update<R: serde::Serialize>(
    projected: ProjectedUpdate<'_, R>,
    cells: Vec<u8>,
) -> Result<(wire::Counter, EncodedChannelFrame), String> {
    let ProjectedUpdate { update, rasters } = projected;
    let revision = update.revision();
    let frame_budget = wire::MAX_ENCODED_FRAME_BYTES
        .checked_sub(cells.len())
        .ok_or("terminal frame exceeds packed cell byte limit")?;
    let images = match &update {
        wire::UpdateRows::Snapshot { images, .. } | wire::UpdateRows::Delta { images, .. } => {
            images
        }
    };
    if rasters.is_empty() {
        return Ok((
            revision,
            EncodedChannelFrame {
                json: encode_json(&update, images, frame_budget)?,
                cells,
                rgba: Vec::new(),
            },
        ));
    }
    #[derive(serde::Serialize)]
    struct BinaryMetadata<'a, R> {
        #[serde(flatten)]
        update: &'a wire::UpdateRows<R>,
        images: Vec<&'a RasterMetadata>,
    }
    let metadata = BinaryMetadata {
        update: &update,
        images: rasters.iter().map(|image| &image.metadata).collect(),
    };
    let json = encode_bounded_json(&metadata, frame_budget)?;
    let bytes = rasters.iter().map(|image| image.rgba.len()).sum::<usize>();
    // Reserve space for the frame metadata and Channel envelope so delivery
    // can reuse this allocation when there is a single image-bearing frame.
    let mut rgba = Vec::with_capacity(bytes + json.len() + 1024);
    for image in rasters {
        rgba.extend_from_slice(image.rgba);
    }
    Ok((revision, EncodedChannelFrame { json, cells, rgba }))
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
    frame_budget: usize,
) -> Result<String, String> {
    let limit = frame_budget + images.iter().map(|image| image.rgba.len()).sum::<usize>();
    if limit > JSON_WORKSPACE_BYTES {
        return Err("terminal frame exceeds its encoding workspace".into());
    }
    encode_bounded_json(value, limit)
}

fn encode_bounded_json(value: &impl serde::Serialize, limit: usize) -> Result<String, String> {
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
    version: u8,
) -> Result<String, String> {
    if (version != wire::IMAGE_VERSION && !wire::supports_packed_rows(version))
        || !(1..=wire::MAX_COLUMNS).contains(&cols)
        || page.row_count > wire::MAX_ROWS
        || page.total_rows > wire::MAX_PROJECTED_HISTORY_ROWS
        || page.offset + page.row_count > page.total_rows
        || text.len() > crate::protocol::MAX_HISTORY_SNAPSHOT_BYTES
        || !text.starts_with(&page.text)
    {
        return Err("invalid history projection bounds".into());
    }
    let _scratch = FrameScratch::acquire().ok_or(ENCODERS_BUSY)?;
    let history = HistoryEncoding {
        owner,
        page,
        cols,
        revision,
        text,
        version,
    };
    if wire::supports_packed_rows(version) {
        encode_history_rows(history, source_at, pack_row)
    } else {
        encode_history_rows(history, source_at, std::convert::identity)
    }
}

struct HistoryEncoding<'a> {
    owner: crate::semantic::Attachment,
    page: &'a HistoryPage,
    cols: usize,
    revision: u64,
    text: &'a str,
    version: u8,
}

fn encode_history_rows<'a, R: serde::Serialize>(
    history: HistoryEncoding<'a>,
    source_at: impl Fn(usize) -> Option<HistoryAnchor>,
    mut encode_row: impl FnMut(wire::RowData<EncodedCell<'a>>) -> R,
) -> Result<String, String> {
    let HistoryEncoding {
        owner,
        page,
        cols,
        revision,
        text,
        version,
    } = history;
    let mut projection = Projection {
        images_enabled: true,
        colors: page.paint_colors(),
        extended_rendition: version == wire::RENDITION_VERSION,
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
                page.underline_color_runs(index),
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
            Ok(wire::HistoryRowWith {
                source: wire::HistorySource {
                    row: counter(row_id)?,
                    column: source.column,
                },
                paint: encode_row(wire::RowData {
                    wrapped: row.wrapped,
                    cells,
                    image_slices,
                }),
                text_offsets,
            })
        })
        .collect::<Result<_, String>>()?;
    let (hyperlinks, rasters) = projection.into_resources();
    let images = rasters.into_iter().map(ProjectedRaster::base64).collect();
    let view = wire::HistoryViewRows {
        owner,
        revision: counter(revision)?,
        version,
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
    encode_json(&view, &view.images, wire::MAX_ENCODED_FRAME_BYTES)
}

struct ProjectedUpdate<'a, R> {
    update: wire::UpdateRows<R>,
    rasters: Vec<ProjectedRaster<'a>>,
}

fn project_update<'a, R>(
    previous: Option<&CellFrame>,
    frame: &'a CellFrame,
    session: &str,
    version: u8,
    binary_rasters: bool,
    mut encode_row: impl FnMut(wire::RowData<EncodedCell<'a>>) -> R,
) -> Result<ProjectedUpdate<'a, R>, String> {
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
    let row_moves = version == DENSE_FRAME_VERSION || wire::supports_row_moves(version);
    let mut projection = Projection {
        images_enabled: version == wire::IMAGE_VERSION || row_moves,
        colors,
        extended_rendition: version == wire::RENDITION_VERSION || version == DENSE_FRAME_VERSION,
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
        let (rows, moves) = if row_moves && changed.len() > 1 {
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
                    Ok(wire::ChangedRowWith {
                        index: index as u16,
                        row: encode_row(wire_row(frame, index, &mut projection)?),
                    })
                })
                .collect::<Result<_, String>>()?;
            let (hyperlinks, images, rasters) = projection.into_update_resources(binary_rasters);
            return Ok(ProjectedUpdate {
                update: wire::UpdateRows::Delta {
                    stamp,
                    base_revision: counter(base.stamp().revision)?,
                    rows,
                    moves,
                    hyperlinks,
                    images,
                },
                rasters,
            });
        }
    }
    let rows = (0..frame.rows().len())
        .map(|index| wire_row(frame, index, &mut projection).map(&mut encode_row))
        .collect::<Result<_, _>>()?;
    let (hyperlinks, images, rasters) = projection.into_update_resources(binary_rasters);
    Ok(ProjectedUpdate {
        update: wire::UpdateRows::Snapshot {
            stamp,
            cols: frame.cols() as u16,
            rows,
            hyperlinks,
            images,
        },
        rasters,
    })
}

#[inline(never)]
fn split_row_moves(
    base: &CellFrame,
    frame: &CellFrame,
    mut rows: Vec<usize>,
) -> (Vec<usize>, Vec<wire::RowMove>) {
    let mut sources = None;
    let mut linear_probes = 2;
    let mut moves: Vec<wire::RowMove> = Vec::new();
    rows.retain(|&to| {
        let matches = |from: usize| from != to && base.shares_row_paint_with(from, frame, to);
        // A region can contain unchanged rows between two moved runs.
        // Try the last displacement even across one of those gaps.
        if let Some(last) = moves.last_mut() {
            if let Some(offset) = to.checked_sub(usize::from(last.to)) {
                let from = usize::from(last.from) + offset;
                if matches(from) {
                    if offset == usize::from(last.count) {
                        last.count += 1;
                    } else {
                        moves.push(wire::RowMove {
                            from: from as u16,
                            to: to as u16,
                            count: 1,
                        });
                    }
                    return false;
                }
            }
        }

        let address = frame.rows()[to].cells.as_ptr();
        // A scroll usually needs one lookup to start its run and one miss
        // at the newly exposed row. Small deltas also avoid sorting the base.
        let from = if linear_probes > 0 {
            linear_probes -= 1;
            base.rows()
                .iter()
                .enumerate()
                .filter(|(_, row)| row.cells.as_ptr() == address)
                .map(|(index, _)| index)
                .find(|&from| matches(from))
        } else {
            let sources = sources.get_or_insert_with(|| {
                let mut entries: Vec<_> = base
                    .rows()
                    .iter()
                    .enumerate()
                    .map(|(index, row)| (row.cells.as_ptr(), index))
                    .collect();
                entries.sort_unstable_by_key(|&(address, _)| address);
                entries
            });
            let start = sources.partition_point(|&(candidate, _)| candidate < address);
            sources[start..]
                .iter()
                .take_while(|&&(candidate, _)| candidate == address)
                .map(|&(_, index)| index)
                .find(|&from| matches(from))
        };
        let Some(from) = from else {
            return true;
        };
        moves.push(wire::RowMove {
            from: from as u16,
            to: to as u16,
            count: 1,
        });
        false
    });
    (rows, moves)
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum EncodedText<'a> {
    Scalar(char),
    Cluster(&'a str),
}

impl EncodedText<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Scalar(character) => character.len_utf8(),
            Self::Cluster(cluster) => cluster.len(),
        }
    }
}

type EncodedCell<'a> = wire::CellData<EncodedText<'a>, RgbColor>;

#[derive(serde::Serialize)]
#[serde(untagged)]
enum EncodedRow<'a> {
    Packed(wire::PackedRow<EncodedText<'a>, RgbColor>),
    Cells(wire::RowData<EncodedCell<'a>>),
    Dense(DenseRow),
}

#[derive(serde::Serialize)]
struct DenseRow {
    wrapped: bool,
    columns: u16,
    // Null on a binary row. Its bytes appear in row order in the Channel.
    cell_bytes: Option<String>,
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attributes: Vec<DenseAttributes>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    image_slices: Vec<wire::ImageSlice>,
}

#[derive(serde::Serialize)]
struct DenseAttributes {
    column: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    underline: Option<wire::Underline>,
    #[serde(skip_serializing_if = "Option::is_none")]
    underline_color: Option<RgbColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hyperlink_id: Option<u16>,
}

fn pack_dense_row<'a>(
    row: wire::RowData<EncodedCell<'a>>,
    binary_cells: Option<&mut Vec<u8>>,
) -> EncodedRow<'a> {
    let row = match pack_row(row) {
        EncodedRow::Cells(row) => row,
        packed => return packed,
    };
    let columns = row.cells.len();
    let binary = binary_cells.is_some();
    let mut base64_input = Vec::new();
    let bytes = binary_cells.unwrap_or(&mut base64_input);
    bytes.reserve(9 * columns);
    let mut text = String::with_capacity(columns);
    let mut attributes = Vec::new();
    for (column, cell) in row.cells.into_iter().enumerate() {
        let text_units = match cell.text {
            EncodedText::Scalar(character) => {
                text.push(character);
                character.len_utf16()
            }
            EncodedText::Cluster(cluster) => {
                text.push_str(cluster);
                cluster.encode_utf16().count()
            }
        };
        let RgbColor(fr, fg, fb) = cell.foreground;
        let RgbColor(br, bg, bb) = cell.background;
        // RGB foreground and background, rendition, width, and the number
        // of UTF-16 code units in the cell's glyph.
        bytes.extend_from_slice(&[
            fr,
            fg,
            fb,
            br,
            bg,
            bb,
            cell.style,
            cell.width,
            text_units as u8,
        ]);
        if cell.underline.is_some() || cell.underline_color.is_some() || cell.hyperlink_id.is_some()
        {
            attributes.push(DenseAttributes {
                column: column as u16,
                underline: cell.underline,
                underline_color: cell.underline_color,
                hyperlink_id: cell.hyperlink_id,
            });
        }
    }
    EncodedRow::Dense(DenseRow {
        wrapped: row.wrapped,
        columns: columns as u16,
        cell_bytes: (!binary).then(|| STANDARD.encode(base64_input)),
        text,
        attributes,
        image_slices: row.image_slices,
    })
}

fn packed_style(cell: &EncodedCell<'_>) -> wire::PackedStyle<RgbColor> {
    wire::PackedStyle {
        foreground: cell.foreground,
        background: cell.background,
        style: cell.style,
        underline: cell.underline,
        underline_color: cell.underline_color,
        hyperlink_id: cell.hyperlink_id,
    }
}

fn row_styles_repeat(cells: &[EncodedCell<'_>]) -> bool {
    let mut sample = [None; 8];
    let stride = cells.len().div_ceil(sample.len()).max(1);
    for (index, cell) in cells.iter().step_by(stride).enumerate() {
        let style = Some(packed_style(cell));
        if sample[..index].contains(&style) {
            return true;
        }
        sample[index] = style;
    }
    // Striding can hit a different style in each long, uniform span. Probe
    // one neighbor per sample before rejecting that kind of row.
    let mut adjacent = 0;
    for (column, style) in (1..cells.len()).step_by(stride).zip(sample) {
        adjacent += usize::from(style == Some(packed_style(&cells[column])));
    }
    adjacent != 0 && 2 * adjacent >= cells.len().div_ceil(stride)
}

fn pack_row(row: wire::RowData<EncodedCell<'_>>) -> EncodedRow<'_> {
    if !row_styles_repeat(&row.cells) {
        return EncodedRow::Cells(row);
    }
    let columns = row.cells.len() as u16;
    // Dense true-color rows benefit less from a palette. Stop collecting
    // early and send the ordinary cell shape for such rows.
    let max_styles = (row.cells.len() / 4).max(1);
    let mut styles = Vec::new();
    let mut last = None;
    // Validated rows fit in the stack buffer. The bounded palette also
    // serves as the lookup table for styles that recur in separate runs.
    let mut indices = [0u16; wire::MAX_COLUMNS];
    let Some(indices) = indices.get_mut(..row.cells.len()) else {
        return EncodedRow::Cells(row);
    };
    for (cell, index) in row.cells.iter().zip(indices.iter_mut()) {
        let paint = packed_style(cell);
        *index = match last {
            Some((previous, index)) if previous == paint => index,
            _ => {
                let index = if let Some(index) = styles.iter().position(|style| *style == paint) {
                    index as u16
                } else {
                    if styles.len() == max_styles {
                        return EncodedRow::Cells(row);
                    }
                    let index = styles.len() as u16;
                    styles.push(paint);
                    index
                };
                last = Some((paint, index));
                index
            }
        };
    }
    let mut runs = Vec::new();
    for (cell, index) in row.cells.into_iter().zip(indices.iter().copied()) {
        if let (1, EncodedText::Scalar(character), Some(last)) =
            (cell.width, &cell.text, runs.last_mut())
        {
            match last {
                wire::PackedRun::Scalars(style, text) if *style == index => {
                    text.push(*character);
                    continue;
                }
                wire::PackedRun::Cell(style, EncodedText::Scalar(first), 1) if *style == index => {
                    let mut text = String::with_capacity(8);
                    text.push(*first);
                    text.push(*character);
                    *last = wire::PackedRun::Scalars(index, text);
                    continue;
                }
                _ => {}
            }
        }
        runs.push(wire::PackedRun::Cell(index, cell.text, cell.width));
    }
    EncodedRow::Packed(wire::PackedRow {
        wrapped: row.wrapped,
        columns,
        styles,
        runs,
        image_slices: row.image_slices,
    })
}

#[derive(serde::Serialize)]
struct RasterMetadata {
    id: wire::Counter,
    width: u32,
    height: u32,
    columns: u16,
    rows: u16,
}

struct ProjectedRaster<'a> {
    metadata: RasterMetadata,
    rgba: &'a [u8],
}

impl ProjectedRaster<'_> {
    fn base64(self) -> wire::ImageResource {
        let RasterMetadata {
            id,
            width,
            height,
            columns,
            rows,
        } = self.metadata;
        wire::ImageResource {
            id,
            width,
            height,
            columns,
            rows,
            rgba: STANDARD.encode(self.rgba),
        }
    }
}

#[derive(Default)]
struct Projection<'a> {
    colors: Option<&'a ColorState>,
    text_bytes: usize,
    hyperlink_bytes: usize,
    last_hyperlink: Option<u16>,
    hyperlinks: BTreeMap<u16, &'a str>,
    images_enabled: bool,
    extended_rendition: bool,
    image_bytes: usize,
    base_images: BTreeSet<u64>,
    images: BTreeMap<u64, ProjectedRaster<'a>>,
}

impl<'a> Projection<'a> {
    fn into_resources(self) -> (Vec<wire::Hyperlink>, Vec<ProjectedRaster<'a>>) {
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

    fn into_update_resources(
        self,
        binary_rasters: bool,
    ) -> (
        Vec<wire::Hyperlink>,
        Vec<wire::ImageResource>,
        Vec<ProjectedRaster<'a>>,
    ) {
        let (links, rasters) = self.into_resources();
        if binary_rasters {
            (links, Vec::new(), rasters)
        } else {
            (
                links,
                rasters.into_iter().map(ProjectedRaster::base64).collect(),
                Vec::new(),
            )
        }
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
                        ProjectedRaster {
                            metadata: RasterMetadata {
                                id,
                                width: raster.width(),
                                height: raster.height(),
                                columns: image.columns(),
                                rows: image.rows(),
                            },
                            rgba: raster.rgba(),
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
        mut underline_runs: &[UnderlineColorRun],
    ) -> Result<Vec<EncodedCell<'a>>, String> {
        let columns = row.cells.len();
        let mut cells = Vec::with_capacity(columns);
        for (column, cell) in row.cells.iter().enumerate() {
            // Columns are visited in order, so each color run only
            // needs to be found once per row.
            while underline_runs
                .first()
                .is_some_and(|run| usize::from(run.end) <= column)
            {
                underline_runs = &underline_runs[1..];
            }
            let underline_color = underline_runs
                .first()
                .filter(|run| usize::from(run.start) <= column)
                .map(|run| paint_rgb(run.color, true, self.colors));
            let flags = cell.flags;
            let width = cell_width(flags, column, columns);
            let text = if width == 0 {
                EncodedText::Scalar(' ')
            } else {
                cluster(column).map_or(EncodedText::Scalar(cell.character), EncodedText::Cluster)
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
                if self.last_hyperlink != Some(id) {
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
                    self.last_hyperlink = Some(id);
                }
                Some(id)
            };
            let mut foreground = paint_rgb(cell.foreground, true, self.colors);
            let mut background = paint_rgb(cell.background, false, self.colors);
            if flags.inverse() {
                std::mem::swap(&mut foreground, &mut background);
            }
            let rendition = if self.extended_rendition {
                (u8::from(flags.concealed()) * wire::STYLE_CONCEALED)
                    | (u8::from(flags.overline()) * wire::STYLE_OVERLINE)
            } else {
                0
            };
            cells.push(wire::CellData {
                text,
                foreground,
                background,
                width,
                style: u8::from(flags.bold())
                    | (u8::from(flags.italic()) << 1)
                    | (u8::from(flags.strikethrough()) << 3)
                    | (u8::from(flags.dim()) << 4)
                    | (u8::from(flags.blink()) << 5)
                    | (u8::from(flags.protected()) << 6)
                    | rendition,
                underline: match flags.underline() {
                    UnderlineStyle::None => None,
                    UnderlineStyle::Single => Some(wire::Underline::Single),
                    UnderlineStyle::Double => Some(wire::Underline::Double),
                    UnderlineStyle::Curly => Some(wire::Underline::Curly),
                    UnderlineStyle::Dotted => Some(wire::Underline::Dotted),
                    UnderlineStyle::Dashed => Some(wire::Underline::Dashed),
                },
                underline_color,
                hyperlink_id,
            });
        }
        Ok(cells)
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
) -> Result<wire::RowData<EncodedCell<'a>>, String> {
    let row = &frame.rows()[index];
    Ok(wire::RowData {
        wrapped: row.wrapped,
        cells: projection.cells(
            row,
            |column| frame.cluster_text(index, column),
            |id| frame.hyperlink_target(id),
            frame.underline_color_runs(index),
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

    fn update(
        previous: Option<&CellFrame>,
        frame: &CellFrame,
        session: &str,
        version: u8,
    ) -> Result<wire::Update, String> {
        let projected = project_update(
            previous,
            frame,
            session,
            version,
            false,
            std::convert::identity,
        )?;
        let data = serde_json::to_vec(&projected.update).map_err(|error| error.to_string())?;
        serde_json::from_slice(&data).map_err(|error| error.to_string())
    }

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

    #[test]
    fn packed_snapshots_group_scalar_runs_and_fall_back_for_dense_colors() {
        let budget = Arc::new(RetainedBytesBudget::new(JSON_WORKSPACE_BYTES));
        let scratch = FrameScratch::reserve_from(&budget).unwrap();
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 8).unwrap();
        terminal
            .feed(b"ABCDEFGH\x1b[2;1H\x1b[31mR\x1b[32mG\x1b[33mY\x1b[0m")
            .unwrap();
        let frame = terminal.frame().unwrap();
        let (_, packed) = scratch
            .encode(None, &frame, "packed", wire::PACKED_VERSION)
            .unwrap();
        let packed: serde_json::Value = serde_json::from_str(&packed).unwrap();
        assert_eq!(
            packed["rows"][0]["runs"],
            serde_json::json!([[0, "ABCDEFGH"]])
        );
        assert_eq!(packed["rows"][0]["styles"].as_array().unwrap().len(), 1);
        let (_, old) = scratch
            .encode(None, &frame, "packed", wire::ROW_MOVE_VERSION)
            .unwrap();
        let old: serde_json::Value = serde_json::from_str(&old).unwrap();
        assert_eq!(packed["rows"][1], old["rows"][1]);
    }

    #[test]
    fn packed_runs_reuse_styles_across_nonadjacent_cells() {
        let mut terminal = jterm_engine::terminal::Terminal::new(1, 16).unwrap();
        terminal
            .feed(b"\x1b[31mABCD\x1b[32mEFGH\x1b[31mIJKL\x1b[32mMNOP")
            .unwrap();
        let (_, encoded) = encode(
            None,
            &terminal.frame().unwrap(),
            "alternating",
            wire::RENDITION_VERSION,
        )
        .unwrap();
        let packed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        let row = &packed["rows"][0];
        assert_eq!(row["styles"].as_array().unwrap().len(), 2);
        assert_eq!(
            row["runs"],
            serde_json::json!([[0, "ABCD"], [1, "EFGH"], [0, "IJKL"], [1, "MNOP"]])
        );
    }

    #[test]
    fn packed_rows_admit_uniform_spans_with_distinct_sampled_styles() {
        let budget = Arc::new(RetainedBytesBudget::new(JSON_WORKSPACE_BYTES));
        let scratch = FrameScratch::reserve_from(&budget).unwrap();
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 128).unwrap();
        for green in 0..16 {
            terminal
                .feed(format!("\x1b[38;2;75;{green};123mABCDEFGH").as_bytes())
                .unwrap();
        }
        let frame = terminal.frame().unwrap();
        let (_, encoded) = scratch
            .encode(None, &frame, "bands", wire::RENDITION_VERSION)
            .unwrap();
        let packed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        let row = &packed["rows"][0];
        assert_eq!(row["styles"].as_array().unwrap().len(), 16);
        assert_eq!(row["runs"].as_array().unwrap().len(), 16);
        assert_eq!(row["runs"][0], serde_json::json!([0, "ABCDEFGH"]));

        // A uniform prefix can pass sampling and then exceed the palette.
        for uniform_prefix in [0, 88] {
            terminal.feed(b"\x1b[H").unwrap();
            for column in 0..128usize {
                let green = column.saturating_sub(uniform_prefix);
                terminal
                    .feed(format!("\x1b[38;2;75;{green};123mA").as_bytes())
                    .unwrap();
            }
            let (_, encoded) = scratch
                .encode(
                    None,
                    &terminal.frame().unwrap(),
                    "pixels",
                    wire::RENDITION_VERSION,
                )
                .unwrap();
            let ordinary: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(ordinary["rows"][0]["cells"].as_array().unwrap().len(), 128);
        }
    }

    #[test]
    fn dense_rows_preserve_glyph_boundaries_colors_and_attributes() {
        let row = || wire::RowData {
            wrapped: true,
            cells: vec![
                EncodedCell {
                    text: EncodedText::Cluster("🙂\u{301}"),
                    foreground: RgbColor(1, 2, 3),
                    background: RgbColor(4, 5, 6),
                    width: 2,
                    style: wire::STYLE_CONCEALED,
                    underline: Some(wire::Underline::Curly),
                    underline_color: Some(RgbColor(7, 8, 9)),
                    hyperlink_id: Some(17),
                },
                EncodedCell {
                    text: EncodedText::Scalar(' '),
                    foreground: RgbColor(10, 11, 12),
                    background: RgbColor(4, 5, 6),
                    width: 0,
                    style: 0,
                    underline: None,
                    underline_color: None,
                    hyperlink_id: None,
                },
            ],
            image_slices: Vec::new(),
        };
        let encoded = serde_json::to_value(pack_dense_row(row(), None)).unwrap();
        let bytes = STANDARD
            .decode(encoded["cell_bytes"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            bytes,
            [
                1,
                2,
                3,
                4,
                5,
                6,
                wire::STYLE_CONCEALED,
                2,
                3,
                10,
                11,
                12,
                4,
                5,
                6,
                0,
                0,
                1
            ]
        );
        assert_eq!(encoded["text"], "🙂\u{301} ");
        assert_eq!(encoded["wrapped"], true);
        assert_eq!(encoded["columns"], 2);
        assert_eq!(
            encoded["attributes"],
            serde_json::json!([{"column":0,"underline":"curly","underline_color":"#070809","hyperlink_id":17}])
        );
        let mut raw = Vec::new();
        let direct = serde_json::to_value(pack_dense_row(row(), Some(&mut raw))).unwrap();
        assert_eq!(raw, bytes);
        let mut expected = encoded;
        expected["cell_bytes"] = serde_json::Value::Null;
        assert_eq!(direct, expected);
    }

    #[test]
    fn a_wide_row_can_pack_more_than_thirty_two_reused_styles() {
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 256).unwrap();
        for green in 0..40 {
            terminal
                .feed(format!("\x1b[38;2;75;{green};123mABCDE").as_bytes())
                .unwrap();
        }
        let (_, encoded) = encode(
            None,
            &terminal.frame().unwrap(),
            "bands",
            wire::RENDITION_VERSION,
        )
        .unwrap();
        let packed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        let row = &packed["rows"][0];
        assert_eq!(row["styles"].as_array().unwrap().len(), 41);
        assert_eq!(row["columns"], 256);
        assert_eq!(row["runs"][39], serde_json::json!([39, "ABCDE"]));
    }

    #[test]
    fn rendition_bits_are_negotiated_for_channel_and_optional_for_compatibility() {
        let budget = Arc::new(RetainedBytesBudget::new(JSON_WORKSPACE_BYTES));
        let scratch = FrameScratch::reserve_from(&budget).unwrap();
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 16).unwrap();
        terminal.feed(b"\x1b[8;53mX\x1b[28mY\x1b[55mZ").unwrap();
        let frame = terminal.frame().unwrap();
        let versioned = |version| {
            let (_, packed) = scratch.encode(None, &frame, "rendition", version).unwrap();
            let packed: serde_json::Value = serde_json::from_str(&packed).unwrap();
            let styles = packed["rows"][0]["styles"].as_array().unwrap();
            styles
                .iter()
                .map(|paint| paint["style"].as_u64().unwrap() as u8)
                .collect::<Vec<_>>()
        };
        assert_eq!(versioned(wire::PACKED_VERSION), [0]);
        assert_eq!(
            versioned(wire::RENDITION_VERSION),
            [
                wire::STYLE_CONCEALED | wire::STYLE_OVERLINE,
                wire::STYLE_OVERLINE,
                0
            ]
        );
        let legacy =
            jterm_protocol::engine_projection::from_engine(&frame, "rendition", 1).unwrap();
        let legacy = serde_json::to_value(legacy).unwrap();
        assert_eq!(legacy["rows"][0][0]["rendition"], 3);
        assert_eq!(legacy["rows"][0][1]["rendition"], 2);
        assert!(legacy["rows"][0][2].get("rendition").is_none());
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
        let (_, binary) = scratch
            .encode_frame(
                Some(&before),
                &displayed,
                "test",
                wire::IMAGE_VERSION,
                BinaryChannelParts {
                    rasters: true,
                    cells: false,
                },
            )
            .unwrap();
        let metadata: serde_json::Value = serde_json::from_str(binary.as_json()).unwrap();
        assert_eq!(metadata["kind"], "delta");
        assert_eq!(metadata["images"][0]["id"], images[0].id.get().to_string());
        assert!(metadata["images"][0].get("rgba").is_none());
        assert_eq!(binary.into_image_bytes(), [255, 0, 0, 255]);
        terminal.feed(b"\x1b[1S").unwrap();
        let moved = terminal.frame().unwrap();
        assert_eq!(
            scratch
                .encode_frame(
                    Some(&displayed),
                    &moved,
                    "test",
                    wire::IMAGE_VERSION,
                    BinaryChannelParts {
                        rasters: true,
                        cells: false
                    }
                )
                .unwrap()
                .1
                .image_bytes(),
            0,
        );
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
    fn separated_move_runs_and_aliased_sources_replay_to_the_full_frame() {
        let mut rows: Vec<_> = ('a'..='p')
            .map(|character| Row {
                cells: Arc::from([TerminalCell {
                    character,
                    ..TerminalCell::default()
                }]),
                wrapped: false,
            })
            .collect();
        rows[6] = rows[2].clone();
        rows[10] = rows[2].clone();
        rows[10].wrapped = true;
        let updated_rows: Vec<_> = [1, 2, 3, 0, 4, 6, 7, 8, 9, 10, 12, 13, 14, 15, 11, 5]
            .iter()
            .map(|&i| rows[i].clone())
            .collect();
        let frame = |revision, rows: Vec<Row>| {
            CellFrame::new(
                Stamp {
                    epoch: 1,
                    revision,
                    layout: 1,
                    screen: Screen::Primary,
                },
                1,
                rows.into(),
                Cursor {
                    row: 0,
                    col: 0,
                    pending_wrap: false,
                    visible: true,
                    shape: CursorShape::Block,
                },
                Limits {
                    max_rows: 16,
                    max_cols: 1,
                    max_cells: 16,
                },
            )
            .unwrap()
        };
        let base = frame(1, rows);
        let next = frame(2, updated_rows);
        let snapshot = |frame| match update(None, frame, "test", wire::ROW_MOVE_VERSION).unwrap() {
            wire::Update::Snapshot { rows, .. } => rows,
            _ => unreachable!(),
        };
        let wire::Update::Delta { rows, moves, .. } =
            update(Some(&base), &next, "test", wire::ROW_MOVE_VERSION).unwrap()
        else {
            panic!("separated runs should still produce a delta");
        };
        assert!(moves.len() > 2);
        let original = snapshot(&base);
        let mut replayed = original.clone();
        for movement in moves {
            let from = movement.from as usize;
            let to = movement.to as usize;
            let count = movement.count as usize;
            replayed[to..to + count].clone_from_slice(&original[from..from + count]);
        }
        for patch in rows {
            replayed[patch.index as usize] = patch.row;
        }
        assert_eq!(replayed, snapshot(&next));
    }

    #[test]
    fn projected_underline_colors_cover_runs_and_the_gaps_between_them() {
        let mut terminal = jterm_engine::terminal::Terminal::new(2, 8).unwrap();
        terminal.enable_underline_colors();
        terminal
            .feed(b"\x1b[4;58;2;1;2;3mAB\x1b[mCD\x1b[4;58;2;4;5;6mE\x1b[58;2;7;8;9mFG\x1b[m")
            .unwrap();
        let frame = terminal.frame().unwrap();
        let wire::Update::Snapshot { rows, .. } =
            update(None, &frame, "test", wire::VERSION).unwrap()
        else {
            unreachable!();
        };
        let colors: Vec<_> = rows[0]
            .cells
            .iter()
            .map(|cell| cell.underline_color.as_deref())
            .collect();
        assert_eq!(
            colors,
            [
                Some("#010203"),
                Some("#010203"),
                None,
                None,
                Some("#040506"),
                Some("#070809"),
                Some("#070809"),
                None,
            ]
        );
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
