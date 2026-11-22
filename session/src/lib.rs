//! Linux-only exploratory session. Shared Rust parser/state is the only VT authority.
//! Not a complete terminal: bounded physical history only; no IME UI, graphics, OSC
//! policy, grapheme composition, or process-tree containment. Never advertise as TERM=xterm.
use jterm_engine::{
    frame::{CellFrame, Cursor, Limits, Row, Screen, Stamp},
    parser::{Event, Parser, ParserLimits, ParserProfile, StringKind},
    protocol_state::ProtocolState,
    screen::{RegionDirection, RegionMutation, ScreenState, VerticalMotion},
    semantics::{self, Dispatch, Effect},
    RawRowId, ScrollbackLine,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    sync::Arc,
};
use unicode_width::UnicodeWidthChar;

const MAX_ROWS: usize = 128;
const MAX_COLS: usize = 256;
const MAX_INPUT: usize = 4096;
const READ_BUDGET: usize = 65536;
fn dimensions(rows: usize, cols: usize) -> Result<(), String> {
    if rows == 0 || cols == 0 || rows > MAX_ROWS || cols > MAX_COLS {
        Err("viewport out of bounds".into())
    } else {
        Ok(())
    }
}

const MAX_HISTORY_ROWS: usize = 1024;
const MAX_COPY_BYTES: usize = MAX_ROWS * MAX_COLS * 4 + MAX_ROWS;
/// Owned immutable primary-history snapshot. Coordinates address original
/// physical columns, not UTF-8 byte indices. It remains valid after live output.
pub struct HistoryPage {
    pub revision: u64,
    pub offset: usize,
    pub total_rows: usize,
    pub row_count: usize,
    pub text: String,
    rows: Vec<HistoryCopyRow>,
}
struct HistoryCopyRow {
    // Physical addressing is unchanged. Only extraction may omit this slot.
    row: Row,
    synthetic_gap: Option<usize>,
}
impl HistoryPage {
    /// Half-open lexicographic range. End may be (row_count, 0) for whole page.
    /// Wide continuations and certified synthetic margin gaps emit no text.
    pub fn selection_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> Result<String, String> {
        let valid = |r: usize, c: usize| {
            r < self.rows.len() && c <= self.rows[r].row.cells.len()
                || r == self.rows.len() && c == 0
        };
        if !valid(start_row, start_col)
            || !valid(end_row, end_col)
            || (start_row, start_col) > (end_row, end_col)
        {
            return Err("selection out of bounds".into());
        }
        let mut text = String::new();
        for row in start_row..self.rows.len().min(end_row.saturating_add(1)) {
            let cells = &self.rows[row].row.cells;
            let start = if row == start_row { start_col } else { 0 };
            let end = if row == end_row { end_col } else { cells.len() };
            for (offset, cell) in cells[start..end].iter().enumerate() {
                if !cell.flags.wide_continuation()
                    && self.rows[row].synthetic_gap != Some(start + offset)
                {
                    text.push(cell.character);
                }
            }
            if row < end_row && !self.rows[row].row.wrapped {
                // Physical trailing blank padding is not copied across a hard line.
                while text.ends_with(' ') {
                    text.pop();
                }
                if row + 1 < self.rows.len() {
                    text.push('\n');
                }
            }
            if text.len() > MAX_COPY_BYTES {
                return Err("selection byte limit".into());
            }
        }
        Ok(text)
    }
}

pub struct Terminal {
    parser: Parser,
    screen: ScreenState,
    protocol: ProtocolState,
    revision: u64,
    layout: u64,
    unsupported: u64,
    unsupported_events: Vec<String>,
    negative_query_replies: u64,
    history: VecDeque<ScrollbackLine>,
    sync_started: Option<std::time::Instant>,
    capture_sync_end: bool,
    completed_sync_frame: Option<CellFrame>,
}
impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Result<Self, String> {
        dimensions(rows, cols)?;
        let mut protocol = ProtocolState::default();
        protocol.modes_mut().insert(7);
        protocol.modes_mut().insert(25);
        Ok(Self {
            parser: Parser::with_bounded_profile(
                ParserLimits {
                    pending_bytes: 4096,
                    kitty_recovery_bytes: 4096,
                },
                ParserProfile::EmberV1,
            )?,
            screen: ScreenState::new(rows, cols),
            protocol,
            revision: 1,
            layout: 1,
            unsupported: 0,
            unsupported_events: Vec::new(),
            negative_query_replies: 0,
            history: VecDeque::new(),
            sync_started: None,
            capture_sync_end: false,
            completed_sync_frame: None,
        })
    }
    /// Paint publication only; parsing, PTY draining and replies remain live.
    /// Repeated begin does not extend the first-begin 250ms safety deadline.
    pub fn publication_held(&self) -> bool {
        self.sync_started
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(250))
    }
    /// Legacy and xterm modifyOtherKeys encoding; Kitty remains unsupported. `name` is a logical browser key or exactly
    /// one already layout-resolved printable Unicode scalar (not a physical key).
    pub fn encode_key(
        &self,
        name: &str,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Result<Vec<u8>, String> {
        use jterm_engine::input::{
            legacy_function_key_sequence, xterm_encode_modify_other_keys, Key, Modifiers,
        };
        if self.protocol.keyboard_enhancement_flags() != 0 {
            return Err("Kitty keyboard flags are unsupported".into());
        }
        let key = match name {
            "ArrowUp" => Key::ArrowUp,
            "ArrowDown" => Key::ArrowDown,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowRight" => Key::ArrowRight,
            "Home" => Key::Home,
            "End" => Key::End,
            "Insert" => Key::Insert,
            "Delete" => Key::Delete,
            "PageUp" => Key::PageUp,
            "PageDown" => Key::PageDown,
            "F1" => Key::F1,
            "F2" => Key::F2,
            "F3" => Key::F3,
            "F4" => Key::F4,
            "F5" => Key::F5,
            "F6" => Key::F6,
            "F7" => Key::F7,
            "F8" => Key::F8,
            "F9" => Key::F9,
            "F10" => Key::F10,
            "F11" => Key::F11,
            "F12" => Key::F12,
            "Enter" => Key::Enter,
            "Escape" => Key::Escape,
            "Backspace" => Key::Backspace,
            "Tab" => Key::Tab,
            _ => {
                let mut chars = name.chars();
                let c = chars.next().ok_or("empty key")?;
                if chars.next().is_some() || c.is_control() {
                    return Err("unsupported key".into());
                }
                Key::Character(c)
            }
        };
        let modifiers = Modifiers {
            shift,
            alt,
            ctrl,
            super_key: false,
        };
        if let Some(sequence) =
            legacy_function_key_sequence(key, modifiers, self.protocol.modes().contains(&1))
        {
            return Ok(sequence.as_bytes().to_vec());
        }
        // Frozen core helper includes Shift at level 2. This host receives
        // layout-resolved text, so follow Frost's committed-text policy: Shift
        // alone does not re-encode uppercase/punctuation. Ctrl/Alt retain Shift
        // in the modifier parameter. Function keys already used legacy CSI above.
        if ctrl || alt {
            let text_character = match key {
                Key::Character(c) => Some(c),
                Key::Enter => Some('\r'),
                Key::Escape => Some('\x1b'),
                Key::Backspace => Some('\x7f'),
                Key::Tab => Some('\t'),
                _ => None,
            };
            if let Some(sequence) = xterm_encode_modify_other_keys(
                key,
                modifiers,
                self.protocol.xterm_modify_other_keys(),
                self.protocol.xterm_format_other_keys(),
                false,
                text_character,
            ) {
                return Ok(sequence.into_bytes());
            }
        }
        let mut bytes = match key {
            Key::Enter if !ctrl && !shift => vec![b'\r'],
            Key::Escape if !ctrl && !shift => vec![0x1b],
            Key::Backspace if !shift => vec![if ctrl { 8 } else { 127 }],
            Key::Tab if !ctrl => {
                if shift {
                    b"\x1b[Z".to_vec()
                } else {
                    vec![b'\t']
                }
            }
            Key::Character(c) if ctrl => {
                let c = c.to_ascii_uppercase();
                let byte = match c {
                    '@' | ' ' | '2' => 0,
                    'A'..='Z' => (c as u8) - b'A' + 1,
                    '[' | '3' => 27,
                    '\\' | '4' => 28,
                    ']' | '5' => 29,
                    '^' | '6' => 30,
                    '_' | '7' => 31,
                    '?' | '8' => 127,
                    _ => return Err("unsupported Ctrl key".into()),
                };
                vec![byte]
            }
            Key::Character(c) => c.to_string().into_bytes(),
            _ => return Err("unsupported legacy modifier combination".into()),
        };
        if alt {
            bytes.insert(0, 0x1b);
        }
        Ok(bytes)
    }
    /// Already-committed IME/text input, not a key event or paste. Frost's
    /// Message::Ime / Ime::Commit path writes committed UTF-8 bytes directly;
    /// xterm modifyOtherKeys only applies to modified key events. Preserve the
    /// complete commit, with no scalar splitting, normalization, modifiers or
    /// bracket markers. The host must suppress duplicate key/input events.
    /// Kitty flags remain unsupported by this spike, including report-all;
    /// do not silently emit a legacy text fallback after that negotiation.
    pub fn encode_text(&self, text: &str) -> Result<Vec<u8>, String> {
        if text.len() > MAX_INPUT {
            return Err("committed text byte limit".into());
        }
        if text.chars().any(char::is_control) {
            return Err("committed text contains control characters".into());
        }
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if self.protocol.keyboard_enhancement_flags() != 0 {
            return Err("IME committed text with Kitty keyboard flags is unsupported".into());
        }
        Ok(text.as_bytes().to_vec())
    }
    /// Wire-size bound includes bracket markers. CRLF/CR/LF become CR. All
    /// other controls except TAB are rejected, including ESC and C1 controls.
    /// In unbracketed mode CR may execute shell lines: host must obtain paste intent.
    pub fn encode_paste(&self, text: &str) -> Result<Vec<u8>, String> {
        if text.len() > MAX_INPUT {
            return Err("paste byte limit".into());
        }
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
        {
            return Err("paste contains control characters".into());
        }
        let normalized = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\r");
        let bytes = if self.protocol.modes().contains(&2004) {
            format!("\x1b[200~{normalized}\x1b[201~").into_bytes()
        } else {
            normalized.into_bytes()
        };
        if bytes.len() > MAX_INPUT {
            return Err("encoded paste byte limit".into());
        }
        Ok(bytes)
    }
    /// Number of explicit negative capability replies, not supported features.
    pub fn negative_query_replies(&self) -> u64 {
        self.negative_query_replies
    }
    pub fn unsupported_count(&self) -> u64 {
        self.unsupported
    }
    /// Up to 16 unique structural identifiers; never stores printable output,
    /// OSC/DCS payloads, clipboard content, or full raw escape sequences.
    pub fn unsupported_events(&self) -> &[String] {
        &self.unsupported_events
    }
    fn record_unsupported_event(&mut self, event: &Event<'_>) {
        if self.unsupported_events.len() >= 16 {
            return;
        }
        let label = match event {
            Event::Csi {
                private_prefix,
                intermediates,
                final_byte,
                params,
                ..
            } => format!(
                "CSI prefix={private_prefix:?} intermediate={:?} final={final_byte} params={:?}",
                &intermediates[..intermediates.len().min(2)],
                &params[..params.len().min(4)]
            ),
            Event::Escape {
                intermediates,
                final_byte,
            } => format!(
                "ESC intermediate={:?} final={final_byte}",
                &intermediates[..intermediates.len().min(2)]
            ),
            Event::Control(byte) => format!("control={byte}"),
            Event::String {
                kind: StringKind::Dcs,
                payload,
                ..
            } => {
                if payload.starts_with(b"+q") {
                    "DCS XTGETTCAP".into()
                } else if payload.starts_with(b"$q") {
                    "DCS DECRQSS".into()
                } else {
                    if payload.as_ref() == b"zz" {
                        "DCS Vim compatibility probe".into()
                    } else {
                        "DCS unknown".into()
                    }
                }
            }
            Event::String { kind, .. } => format!("string={kind:?}"),
            Event::RejectApc { .. } => "rejected APC".into(),
            Event::Print(_) => "zero-width scalar / grapheme composition unsupported".into(),
            _ => "other unsupported event".into(),
        };
        if !self.unsupported_events.contains(&label) {
            self.unsupported_events.push(label);
        }
    }
    fn unsupported(&mut self) {
        self.unsupported = self.unsupported.saturating_add(1);
    }
    fn capture_history(&mut self, effect: Option<RegionMutation>) {
        if let Some(effect) = effect {
            if !self.screen.use_alt_buffer
                && effect.direction == RegionDirection::Up
                && effect.top == 0
                && effect.bottom + 1 == self.screen.grid.rows()
            {
                for removed in effect.removed {
                    if let Some(line) = removed.captured {
                        if self.history.len() == MAX_HISTORY_ROWS {
                            self.history.pop_front();
                        }
                        self.history.push_back(line);
                    }
                }
            }
        }
    }
    fn history_admitted(&self) -> bool {
        !self.screen.use_alt_buffer
            && self.screen.scroll_region_top == 0
            && self.screen.scroll_region_bottom + 1 == self.screen.grid.rows()
    }
    fn motion(&mut self, m: VerticalMotion) {
        let capture = self.history_admitted();
        let effect = self
            .screen
            .vertical_motion(m, capture, || RawRowId::UNTRACKED);
        self.capture_history(effect.region);
    }
    pub fn history_rows(&self) -> usize {
        if self.screen.use_alt_buffer {
            0
        } else {
            self.history.len()
        }
    }
    /// Newest-relative history pagination, oldest-to-newest within each page.
    /// Historical widths are retained across resize; there is no history reflow.
    /// Resize truncations are not archived. Alternate screen cannot read history.
    pub fn history_page(&self, offset: usize, limit: usize) -> Result<HistoryPage, String> {
        let total_rows = self.history_rows();
        if limit == 0 || limit > MAX_ROWS || offset > MAX_HISTORY_ROWS {
            return Err("history page out of bounds".into());
        }
        let offset = offset.min(total_rows);
        let end = total_rows - offset;
        let start = end.saturating_sub(limit);
        let rows: Vec<HistoryCopyRow> = self
            .history
            .iter()
            .skip(start)
            .take(end - start)
            .map(|line| HistoryCopyRow {
                row: Row {
                    cells: Arc::from(line.decompress()),
                    wrapped: line.is_wrapped,
                },
                synthetic_gap: line.synthetic_margin_gap(),
            })
            .collect();
        let mut page = HistoryPage {
            revision: self.revision,
            offset,
            total_rows,
            row_count: rows.len(),
            text: String::new(),
            rows,
        };
        page.text = page.selection_text(0, 0, page.row_count, 0)?;
        Ok(page)
    }
    fn print(&mut self, ch: char) {
        let width = ch.width().unwrap_or(0);
        if width == 0 || width > 2 {
            // The shared cell stores one scalar, not a grapheme cluster. Never
            // overwrite its base or consume a column for an unrepresentable mark.
            self.record_unsupported_event(&Event::Print(ch));
            self.unsupported();
            return;
        }
        let wrap = self.protocol.modes().contains(&7);
        if self.screen.take_pending_wrap(wrap) {
            self.motion(VerticalMotion::Wrap);
        }
        if self.screen.prepare_character_width(width, wrap) {
            let capture = self.history_admitted();
            let effect = self
                .screen
                .vertical_early_wide_wrap(width, capture, || RawRowId::UNTRACKED);
            self.capture_history(effect.region);
        }
        let translated = self.screen.translate_char(ch);
        self.screen.write_char_in_row(translated, width, wrap);
        self.screen.last_printed_char = Some(translated);
    }
    // Host mappings follow Ember parser mode dispatch and the shared ScreenState
    // contracts at pinned jterm_core 2b37583. No byte-level parser lives in this host.
    fn host_event(&mut self, event: &Event<'_>) -> bool {
        match event {
            Event::Escape {
                intermediates,
                final_byte,
            } if intermediates.is_empty() => match final_byte {
                b'D' => self.motion(VerticalMotion::LineFeed),
                b'E' => {
                    self.screen.begin_next_line();
                    self.motion(VerticalMotion::LineFeed);
                }
                b'M' => self.motion(VerticalMotion::ReverseIndex),
                _ => return false,
            },
            Event::Escape {
                intermediates,
                final_byte: b'8',
            } if intermediates == b"#" => {
                self.screen.screen_alignment_test();
            }
            Event::Csi {
                params,
                private_prefix,
                intermediates,
                final_byte,
                ..
            } if intermediates.is_empty() => {
                let first = params.first().copied().unwrap_or(0);
                let count = usize::from(first.max(1));
                match (*private_prefix, *final_byte) {
                    (Some(b'?'), b'h' | b'l') => {
                        let set = *final_byte == b'h';
                        for &mode in params.iter() {
                            match mode {
                                47 | 1047 | 1049 => {
                                    if set {
                                        if self
                                            .screen
                                            .enter_alternate_screen(
                                                mode == 1049,
                                                mode != 47,
                                                &self.protocol,
                                            )
                                            .is_some()
                                        {
                                            self.protocol.switch_screen();
                                        }
                                    } else if let Some(effect) =
                                        self.screen.leave_alternate_screen(mode == 1049)
                                    {
                                        self.protocol.switch_screen();
                                        if let Some(wrap) = effect.restore_autowrap {
                                            if wrap {
                                                self.protocol.modes_mut().insert(7);
                                            } else {
                                                self.protocol.modes_mut().remove(&7);
                                            }
                                        }
                                    }
                                }
                                1048 => {
                                    if set {
                                        self.screen.save_cursor_state(&self.protocol);
                                    } else {
                                        self.screen.restore_cursor_state(&mut self.protocol);
                                    }
                                }
                                6 => {
                                    self.screen.origin_mode = set;
                                    self.screen.set_cursor_position(1, 1);
                                }
                                7 => {
                                    if !set {
                                        self.screen.pending_wrap = false;
                                    }
                                }
                                2026 => {
                                    if set {
                                        self.sync_started
                                            .get_or_insert_with(std::time::Instant::now);
                                    } else {
                                        self.sync_started = None;
                                    }
                                    continue;
                                }
                                1 | 25 | 2004 => (),
                                _ => {
                                    self.unsupported();
                                    continue;
                                }
                            }
                            if set {
                                self.protocol.modes_mut().insert(mode);
                            } else {
                                self.protocol.modes_mut().remove(&mode);
                            }
                        }
                    }
                    (None, b'h' | b'l') => {
                        for &mode in params.iter() {
                            if mode == 4 {
                                self.screen.insert_mode = *final_byte == b'h';
                            } else {
                                self.unsupported();
                            }
                        }
                    }
                    (None | Some(b'?'), b'J') => {
                        let selective = *private_prefix == Some(b'?');
                        let rows = self.screen.grid.rows();
                        let cols = self.screen.grid.cols();
                        let cursor = self.screen.cursor_row;
                        match first {
                            0..=2 => {
                                for row in 0..rows {
                                    let span = match first {
                                        0 if row > cursor => Some((0, cols)),
                                        0 if row == cursor => Some((self.screen.cursor_col, cols)),
                                        1 if row < cursor => Some((0, cols)),
                                        1 if row == cursor => Some((0, self.screen.cursor_col + 1)),
                                        2 => Some((0, cols)),
                                        _ => None,
                                    };
                                    if let Some((start, end)) = span {
                                        self.screen.erase_span(row, start, end, selective);
                                    }
                                }
                                self.screen.pending_wrap = false;
                            }
                            // Alternate applications cannot erase primary history.
                            3 if !selective => {
                                if !self.screen.use_alt_buffer {
                                    self.history.clear();
                                }
                            }
                            _ => self.unsupported(),
                        }
                    }
                    (None, b'L' | b'M') => {
                        let top = self.screen.cursor_row;
                        let bottom = self.screen.scroll_region_bottom;
                        if top >= self.screen.scroll_region_top && top <= bottom {
                            let direction = if *final_byte == b'L' {
                                RegionDirection::Down
                            } else {
                                RegionDirection::Up
                            };
                            self.screen
                                .edit_lines(direction, top, bottom, count, || RawRowId::UNTRACKED);
                        }
                    }
                    (None, b'S' | b'T') => {
                        let top = self.screen.scroll_region_top;
                        let bottom = self.screen.scroll_region_bottom;
                        let direction = if *final_byte == b'S' {
                            RegionDirection::Up
                        } else {
                            RegionDirection::Down
                        };
                        for _ in 0..count.min(bottom - top + 1) {
                            let capture =
                                self.history_admitted() && direction == RegionDirection::Up;
                            let effect = self.screen.scroll_region_one(
                                direction,
                                top,
                                bottom,
                                capture,
                                || RawRowId::UNTRACKED,
                            );
                            self.capture_history(effect);
                        }
                    }
                    (None, b'b') => {
                        if let Some(ch) = self.screen.last_printed_char {
                            for _ in 0..count {
                                self.print(ch);
                            }
                        }
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }
    /// Bounded feed. Replies are returned to the owning PTY, not sent to any other service.
    pub fn feed(&mut self, input: &[u8]) -> Result<Vec<u8>, String> {
        if input.len() > READ_BUDGET {
            return Err("feed byte limit".into());
        }
        self.revision = self.revision.checked_add(1).ok_or("revision exhausted")?;
        self.parser.begin_feed();
        let mut at = 0;
        let mut replies = Vec::new();
        let mut steps = 0;
        while at < input.len() {
            steps += 1;
            if steps > input.len() * 2 + 16 {
                return Err("parser made no progress".into());
            }
            let step = self.parser.next(&input[at..]);
            at += step.consumed;
            let Some(event) = step.event else { continue };
            // Aether has no DA identity contract. A newer shared legacy
            // dispatcher answers DA1/DA2 as Ember; do not inherit that identity.
            // As before, count unsupported CSI c, but leave ESC c (RIS) alone.
            if matches!(
                &event,
                Event::Csi {
                    final_byte: b'c',
                    ..
                }
            ) {
                let before = self.unsupported;
                self.unsupported();
                if self.unsupported != before {
                    self.record_unsupported_event(&event);
                }
                continue;
            }
            // Local adapter compatibility guard retained from the 57ed89a baseline: it accepted
            // nonempty CSI intermediates for SGR. Vim deliberately sends 0%m
            // as an unknown-sequence probe; it must not reset drawing state.
            // This does not claim the shared core itself has been corrected.
            if matches!(&event, Event::Csi { intermediates, final_byte: b'm', .. } if !intermediates.is_empty())
            {
                self.unsupported();
                self.record_unsupported_event(&event);
                continue;
            }
            // Reflect the effective hold only for the shared mode-query reducer;
            // do not sample the clock or touch the mode set on ordinary output.
            let sync_query = matches!(&event, Event::Csi {
                params,
                private_prefix: Some(b'?'),
                intermediates,
                final_byte: b'p',
                ..
            } if intermediates == b"$" && params.first() == Some(&2026));
            if sync_query && self.publication_held() {
                self.protocol.modes_mut().insert(2026);
            }
            let capture_end = self.capture_sync_end
                && self.sync_started.is_some()
                && matches!(&event, Event::Csi { private_prefix: Some(b'?'), intermediates, final_byte: b'l', params, .. }
                    if intermediates.is_empty() && params.contains(&2026));
            let dispatch = semantics::dispatch_local(&mut self.screen, &mut self.protocol, &event);
            if sync_query {
                self.protocol.modes_mut().remove(&2026);
            }
            match dispatch {
                Dispatch::Handled(effects) => {
                    for effect in effects {
                        // Row edits are already applied by ScreenState. Effects
                        // describe host dirty/provenance work; this host publishes
                        // full immutable frames and has no provenance/history.
                        if let Effect::Reply(bytes) = effect {
                            if replies.len() + bytes.len() > READ_BUDGET {
                                return Err("reply limit".into());
                            }
                            replies.extend(bytes);
                        }
                    }
                }
                Dispatch::Unhandled => match event {
                    Event::Print(ch) => self.print(ch),
                    Event::Ascii(bytes) => {
                        for &byte in bytes {
                            self.print(byte as char);
                        }
                    }
                    Event::Control(10..=12) => self.motion(VerticalMotion::LineFeed),
                    Event::Control(0) | Event::Boundary => (),
                    _ => {
                        let before = self.unsupported;
                        if matches!(&event, Event::String { kind: StringKind::Dcs, payload, .. } if payload.starts_with(b"+q") && payload.len() <= 4096)
                        {
                            // XTerm ctlseqs XTGETTCAP: an unknown first name
                            // ends the list with DCS 0+r ST. No capability table
                            // is advertised here, and no payload is reflected.
                            const NEGATIVE: &[u8] = b"\x1bP0+r\x1b\\";
                            if replies.len() + NEGATIVE.len() > READ_BUDGET {
                                return Err("reply limit".into());
                            }
                            replies.extend_from_slice(NEGATIVE);
                            self.negative_query_replies =
                                self.negative_query_replies.saturating_add(1);
                            self.unsupported();
                        } else if !self.host_event(&event) {
                            self.unsupported();
                        }
                        if self.unsupported != before {
                            self.record_unsupported_event(&event);
                        }
                    }
                },
            }
            if capture_end {
                // Exactly one bounded immutable frame per opted-in PTY poll.
                // Reserve a distinct live revision even when no more bytes arrive.
                let next = self.revision.checked_add(1).ok_or("revision exhausted")?;
                self.completed_sync_frame = Some(self.frame()?);
                self.revision = next;
                self.capture_sync_end = false;
            }
        }
        Ok(replies)
    }
    pub fn resize(&mut self, rows: usize, cols: usize) -> Result<(), String> {
        self.resize_with(rows, cols, || Ok(()))
    }
    // Validate all fallible local work before touching PTY geometry. Apply the
    // infallible shared geometry mutation only after the kernel accepts resize.
    fn resize_with(
        &mut self,
        rows: usize,
        cols: usize,
        resize_pty: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        dimensions(rows, cols)?;
        let layout = self.layout.checked_add(1).ok_or("layout exhausted")?;
        let revision = self.revision.checked_add(1).ok_or("revision exhausted")?;
        resize_pty()?;
        self.screen
            .resize_geometry(rows, cols, 0, || RawRowId::UNTRACKED);
        self.layout = layout;
        self.revision = revision;
        Ok(())
    }
    pub fn frame(&self) -> Result<CellFrame, String> {
        let rows = (0..self.screen.grid.rows())
            .map(|r| Row {
                cells: Arc::from(self.screen.grid[r].to_vec()),
                wrapped: self.screen.grid.row_wrapped[r],
            })
            .collect::<Vec<_>>();
        CellFrame::new(
            Stamp {
                epoch: 1,
                revision: self.revision,
                layout: self.layout,
                screen: if self.screen.use_alt_buffer {
                    Screen::Alternate
                } else {
                    Screen::Primary
                },
            },
            self.screen.grid.cols(),
            rows.into(),
            Cursor {
                row: self.screen.cursor_row,
                col: self.screen.cursor_col,
                pending_wrap: self.screen.pending_wrap,
                visible: self.protocol.modes().contains(&25),
                shape: self.screen.cursor_shape,
            },
            Limits {
                max_rows: MAX_ROWS,
                max_cols: MAX_COLS,
                max_cells: MAX_ROWS * MAX_COLS,
            },
        )
        .map_err(|e| format!("{e:?}"))
    }
    pub fn visible_text(&self) -> String {
        (0..self.screen.grid.rows())
            .map(|r| {
                self.screen.grid[r]
                    .iter()
                    .map(|c| c.character)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Bounded EINTR retry; zero progress means the caller retains all bytes.
fn bounded_write(writer: &mut dyn Write, input: &[u8]) -> std::io::Result<usize> {
    for _ in 0..8 {
        match writer.write(input) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(0),
            Ok(0) if !input.is_empty() => return Err(std::io::ErrorKind::WriteZero.into()),
            result => return result,
        }
    }
    Ok(0)
}

#[cfg(target_os = "linux")]
mod reaper {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    };
    pub(super) const CAPACITY: usize = 32;
    #[derive(Default)]
    pub(super) struct Ticket {
        pub reaped: AtomicBool,
        pub pending: AtomicBool,
    }
    pub(super) fn pidfd(pid: u32) -> Result<OwnedFd, String> {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
        }
    }
    fn signal(fd: &OwnedFd, value: i32) -> Result<(), String> {
        let rc = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                value,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
    pub(super) fn check_child_wait_ownership() -> Result<(), String> {
        let mut action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
        if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), action.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let action = unsafe { action.assume_init() };
        if action.sa_sigaction != libc::SIG_DFL || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
            return Err(
                "exclusive child wait ownership requires default SIGCHLD disposition".into(),
            );
        }
        Ok(())
    }
    struct OwnedChild {
        child: Box<dyn Child + Send + Sync>,
        pidfd: Option<OwnedFd>,
        signalled: bool,
        ticket: Arc<Ticket>,
    }
    enum Slot {
        Free,
        Reserved,
        Pending(OwnedChild),
        Processing,
    }
    pub(super) struct Reaper {
        slots: Mutex<Vec<Slot>>,
        healthy: AtomicBool,
    }
    pub(super) struct Lease {
        owner: Arc<Reaper>,
        index: usize,
        active: bool,
        pub ticket: Arc<Ticket>,
    }
    impl Reaper {
        fn table() -> Arc<Self> {
            Arc::new(Self {
                slots: Mutex::new((0..CAPACITY).map(|_| Slot::Free).collect()),
                healthy: AtomicBool::new(true),
            })
        }
        pub fn reserve(self: &Arc<Self>) -> Result<Lease, String> {
            if !self.healthy.load(Ordering::Acquire) {
                return Err("reaper unavailable".into());
            }
            let mut slots = self.slots.lock().map_err(|_| "reaper poisoned")?;
            if !self.healthy.load(Ordering::Acquire) {
                return Err("reaper unavailable".into());
            }
            let index = slots
                .iter()
                .position(|slot| matches!(slot, Slot::Free))
                .ok_or("session capacity exhausted")?;
            slots[index] = Slot::Reserved;
            Ok(Lease {
                owner: self.clone(),
                index,
                active: true,
                ticket: Arc::new(Ticket::default()),
            })
        }
        fn tick(&self) {
            if !self.healthy.load(Ordering::Acquire) {
                return;
            }
            for index in 0..CAPACITY {
                let mut owned = {
                    let Ok(mut slots) = self.slots.lock() else {
                        self.healthy.store(false, Ordering::Release);
                        return;
                    };
                    if !matches!(slots[index], Slot::Pending(_)) {
                        continue;
                    }
                    match std::mem::replace(&mut slots[index], Slot::Processing) {
                        Slot::Pending(child) => child,
                        _ => unreachable!(),
                    }
                };
                // No mutex spans OS calls. Retain ownership on errors/panics.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match owned.child.try_wait() {
                        Ok(Some(_)) => true,
                        Ok(None) => {
                            if !owned.signalled {
                                if let Some(fd) = &owned.pidfd {
                                    if signal(fd, libc::SIGKILL).is_ok() {
                                        owned.signalled = true;
                                    } else {
                                        owned.pidfd.take(); // Permanent observe-only quarantine.
                                    }
                                }
                            }
                            false
                        }
                        // ECHILD and all other errors are quarantine, not evidence of life.
                        Err(_) => {
                            owned.pidfd.take();
                            false
                        }
                    }
                }));
                let reaped = matches!(result, Ok(true));
                if result.is_err() {
                    self.healthy.store(false, Ordering::Release);
                }
                match self.slots.lock() {
                    Ok(mut slots) => {
                        if reaped {
                            owned.ticket.reaped.store(true, Ordering::Release);
                            owned.ticket.pending.store(false, Ordering::Release);
                            slots[index] = Slot::Free;
                        } else {
                            slots[index] = Slot::Pending(owned);
                        }
                    }
                    Err(poison) => {
                        self.healthy.store(false, Ordering::Release);
                        // Preserve owner/slot even if the table is poisoned; never free it.
                        poison.into_inner()[index] = Slot::Pending(owned);
                        return;
                    }
                }
                if !self.healthy.load(Ordering::Acquire) {
                    return;
                }
            }
        }
    }
    impl Lease {
        pub fn handoff(mut self, child: Box<dyn Child + Send + Sync>, pidfd: Option<OwnedFd>) {
            self.ticket.pending.store(true, Ordering::Release);
            let owned = OwnedChild {
                child,
                pidfd,
                signalled: false,
                ticket: self.ticket.clone(),
            };
            match self.owner.slots.lock() {
                Ok(mut slots) => slots[self.index] = Slot::Pending(owned),
                Err(poison) => {
                    self.owner.healthy.store(false, Ordering::Release);
                    poison.into_inner()[self.index] = Slot::Pending(owned);
                }
            }
            self.active = false;
        }
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            if self.active {
                if let Ok(mut slots) = self.owner.slots.lock() {
                    if matches!(slots[self.index], Slot::Reserved) {
                        slots[self.index] = Slot::Free;
                    }
                }
            }
        }
    }
    fn initialize(
        start_worker: impl FnOnce(Arc<Reaper>) -> Result<(), String>,
    ) -> Result<Arc<Reaper>, String> {
        // No child exists yet. A failed worker/capability probe cannot orphan one.
        let probe = pidfd(std::process::id())?;
        signal(&probe, 0)?;
        drop(probe);
        let reaper = Reaper::table();
        start_worker(reaper.clone())?;
        Ok(reaper)
    }
    pub(super) fn global() -> Result<Arc<Reaper>, String> {
        static GLOBAL: OnceLock<Result<Arc<Reaper>, String>> = OnceLock::new();
        GLOBAL
            .get_or_init(|| {
                initialize(|worker| {
                    std::thread::Builder::new()
                        .name("jterm-child-reaper".into())
                        .spawn(move || loop {
                            worker.tick();
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        })
                        .map_err(|e| format!("reaper startup: {e}"))?;
                    Ok(())
                })
            })
            .clone()
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicUsize;
        #[derive(Debug)]
        struct FakeChild {
            state: Arc<AtomicUsize>,
        }
        impl portable_pty::ChildKiller for FakeChild {
            fn kill(&mut self) -> std::io::Result<()> {
                panic!("portable kill forbidden");
            }
            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                Box::new(Self {
                    state: self.state.clone(),
                })
            }
        }
        impl Child for FakeChild {
            fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
                match self.state.load(Ordering::Acquire) {
                    0 => Ok(None),
                    1 => Err(std::io::Error::from_raw_os_error(libc::ECHILD)),
                    2 => Ok(Some(portable_pty::ExitStatus::with_exit_code(0))),
                    _ => panic!("injected child polling panic"),
                }
            }
            fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
                panic!("blocking wait forbidden");
            }
            fn process_id(&self) -> Option<u32> {
                None
            }
        }
        fn child(state: &Arc<AtomicUsize>) -> Box<dyn Child + Send + Sync> {
            Box::new(FakeChild {
                state: state.clone(),
            })
        }
        fn harmless_invalid_pidfd() -> OwnedFd {
            std::fs::File::open("/dev/null").unwrap().into()
        }
        #[test]
        fn startup_failure_publishes_no_reaper_or_child() {
            assert!(initialize(|_| Err("injected worker startup failure".into())).is_err());
        }
        #[test]
        fn poisoned_worker_table_retains_handoff_and_capacity() {
            let owner = Reaper::table();
            let lease = owner.reserve().unwrap();
            let index = lease.index;
            let ticket = lease.ticket.clone();
            let poisoned = owner.clone();
            assert!(std::thread::spawn(move || {
                let _guard = poisoned.slots.lock().unwrap();
                panic!("injected table poison");
            })
            .join()
            .is_err());
            let state = Arc::new(AtomicUsize::new(2));
            lease.handoff(child(&state), None);
            owner.tick();
            assert!(owner.reserve().is_err());
            assert!(ticket.pending.load(Ordering::Acquire));
            assert!(!ticket.reaped.load(Ordering::Acquire));
            let slots = match owner.slots.lock() {
                Err(poison) => poison.into_inner(),
                Ok(_) => panic!("expected poison"),
            };
            assert!(matches!(slots[index], Slot::Pending(_)));
        }
        #[test]
        fn permanent_unreaped_children_exhaust_exact_fixed_capacity() {
            let owner = Reaper::table();
            let state = Arc::new(AtomicUsize::new(0));
            for _ in 0..CAPACITY {
                owner.reserve().unwrap().handoff(child(&state), None);
            }
            for _ in 0..3 {
                owner.tick();
            }
            assert!(owner.reserve().is_err());
            assert_eq!(owner.slots.lock().unwrap().len(), CAPACITY);
            state.store(2, Ordering::Release);
            owner.tick();
            assert!(owner.reserve().is_ok());
        }
        #[test]
        fn setup_failure_releases_reservation_before_any_child() {
            let owner = Reaper::table();
            fn failed_setup(owner: &Arc<Reaper>) -> Result<(), String> {
                let _lease = owner.reserve()?;
                Err("injected openpty/spawn failure".into())
            }
            for _ in 0..CAPACITY * 2 {
                assert!(failed_setup(&owner).is_err());
            }
            let leases: Vec<_> = (0..CAPACITY).map(|_| owner.reserve().unwrap()).collect();
            assert!(owner.reserve().is_err());
            drop(leases);
            assert!(owner.reserve().is_ok());
        }
        #[test]
        fn wait_error_and_signal_failure_remove_signal_authority_but_keep_slots() {
            for initial in [0, 1] {
                let owner = Reaper::table();
                let state = Arc::new(AtomicUsize::new(initial));
                let lease = owner.reserve().unwrap();
                let index = lease.index;
                let ticket = lease.ticket.clone();
                lease.handoff(child(&state), Some(harmless_invalid_pidfd()));
                owner.tick();
                {
                    let slots = owner.slots.lock().unwrap();
                    assert!(matches!(&slots[index], Slot::Pending(c) if c.pidfd.is_none()));
                }
                state.store(0, Ordering::Release);
                owner.tick();
                assert!(ticket.pending.load(Ordering::Acquire));
                assert!(!ticket.reaped.load(Ordering::Acquire));
                state.store(2, Ordering::Release);
                owner.tick();
                assert!(ticket.reaped.load(Ordering::Acquire));
                let next = owner.reserve().unwrap();
                assert_eq!(next.index, index);
                assert!(!next.ticket.reaped.load(Ordering::Acquire));
                assert!(ticket.reaped.load(Ordering::Acquire));
            }
        }
        #[test]
        fn poll_panic_quarantines_owner_and_rejects_new_spawn() {
            let owner = Reaper::table();
            let state = Arc::new(AtomicUsize::new(3));
            let lease = owner.reserve().unwrap();
            let ticket = lease.ticket.clone();
            lease.handoff(child(&state), None);
            owner.tick();
            assert!(owner.reserve().is_err());
            assert!(ticket.pending.load(Ordering::Acquire));
            assert!(!ticket.reaped.load(Ordering::Acquire));
            state.store(2, Ordering::Release);
            owner.tick();
            assert!(ticket.pending.load(Ordering::Acquire));
        }
    }
}

/// Fixed /bin/sh demo host, no arbitrary executable/path API. Poll is nonblocking.
/// Only the direct shell is owned/reaped: do not use as a production session manager.
#[cfg(target_os = "linux")]
pub struct Session {
    master: Option<Box<dyn MasterPty + Send>>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    pidfd: Option<std::os::fd::OwnedFd>,
    lease: Option<reaper::Lease>,
    reap_ticket: Arc<reaper::Ticket>,
    terminal: Terminal,
    replies: VecDeque<u8>,
    holding_replies: bool,
    exited: bool,
    closed: bool,
    read_eof: bool,
    write_error: Option<String>,
    dropped_reply_bytes: usize,
}
#[cfg(target_os = "linux")]
impl Session {
    pub fn spawn(rows: usize, cols: usize) -> Result<Self, String> {
        let terminal = Terminal::new(rows, cols)?;
        let lease = reaper::global()?.reserve()?;
        reaper::check_child_wait_ownership()?;
        let reap_ticket = lease.ticket.clone();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let fd = pair
            .master
            .as_raw_fd()
            .ok_or("nonblocking PTY unsupported")?;
        // The cloned descriptors share the same open-file status flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-i");
        command.env("TERM", "dumb");
        command.env("PS1", "$ ");
        command.env("ENV", "/dev/null");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let pidfd = match child
            .process_id()
            .ok_or("child has no process id".to_string())
            .and_then(reaper::pidfd)
        {
            Ok(fd) => fd,
            Err(error) => {
                drop(writer);
                drop(reader);
                drop(pair.master);
                lease.handoff(child, None); // Observe-only quarantine, never raw-PID fallback.
                return Err(format!("child pidfd unavailable: {error}"));
            }
        };
        Ok(Self {
            master: Some(pair.master),
            reader: Some(reader),
            writer: Some(writer),
            child: Some(child),
            pidfd: Some(pidfd),
            lease: Some(lease),
            reap_ticket,
            terminal,
            replies: VecDeque::new(),
            holding_replies: false,
            exited: false,
            closed: false,
            read_eof: false,
            write_error: None,
            dropped_reply_bytes: 0,
        })
    }
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }
    pub fn is_exited(&self) -> bool {
        self.exited
            || self
                .reap_ticket
                .reaped
                .load(std::sync::atomic::Ordering::Acquire)
    }
    /// Close returns after handoff; this remains true until actual reaping.
    pub fn cleanup_pending(&self) -> bool {
        self.reap_ticket
            .pending
            .load(std::sync::atomic::Ordering::Acquire)
    }
    /// Exit is not output completion: poll until this becomes true.
    /// Descendants holding the slave open can prevent completion indefinitely.
    pub fn is_drained(&self) -> bool {
        self.is_exited() && self.read_eof
    }
    /// Sticky write failure. Read-side polling remains available for final output.
    pub fn write_error(&self) -> Option<&str> {
        self.write_error.as_deref()
    }
    pub fn dropped_reply_bytes(&self) -> usize {
        self.dropped_reply_bytes
    }
    fn fail_writer(&mut self, message: String) {
        if self.write_error.is_none() {
            self.write_error = Some(message);
        }
        self.dropped_reply_bytes = self.dropped_reply_bytes.saturating_add(self.replies.len());
        self.replies.clear();
        self.holding_replies = false;
        self.writer.take();
    }
    pub fn write(&mut self, input: &[u8]) -> Result<usize, String> {
        if self.closed || self.exited {
            return Err("session is closed".into());
        }
        if input.len() > MAX_INPUT {
            return Err("input byte limit".into());
        }
        if self.holding_replies {
            return Ok(0);
        }
        self.flush_replies();
        if let Some(error) = &self.write_error {
            return Err(error.clone());
        }
        if !self.replies.is_empty() {
            return Ok(0);
        }
        self.write_bytes(input)
    }

    /// Bridge-only continuation of the already-started, exact logical head.
    /// It does not flush replies into that event, and cannot offer a new head.
    #[doc(hidden)]
    pub fn continue_partial_input(
        &mut self,
        suffix: &[u8],
        accepted_prefix: std::num::NonZeroU16,
        remaining: usize,
    ) -> Result<usize, String> {
        if self.closed || self.exited {
            return Err("session is closed".into());
        }
        if !self.holding_replies
            || remaining == 0
            || remaining > MAX_INPUT - usize::from(accepted_prefix.get()).min(MAX_INPUT)
            || suffix.len() != remaining.min(4096)
        {
            return Err("partial input continuation size".into());
        }
        if let Some(error) = &self.write_error {
            return Err(error.clone());
        }
        self.write_bytes(suffix)
    }

    fn write_bytes(&mut self, input: &[u8]) -> Result<usize, String> {
        // Caller owns unaccepted suffix. Interrupted/WouldBlock consume no input.
        match bounded_write(self.writer.as_mut().ok_or("writer closed")?, input) {
            Ok(n) => Ok(n),
            Err(e) => {
                let message = e.to_string();
                self.fail_writer(message.clone());
                Err(message)
            }
        }
    }
    fn flush_replies(&mut self) {
        // Finite work even if a backend repeatedly interrupts or accepts one byte.
        for _ in 0..64 {
            if self.replies.is_empty() {
                break;
            }
            let result = match self.writer.as_mut() {
                Some(writer) => bounded_write(writer, self.replies.make_contiguous()),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "writer closed",
                )),
            };
            match result {
                Ok(0) => break,
                Ok(n) => {
                    self.replies.drain(..n);
                }
                Err(e) => {
                    self.fail_writer(e.to_string());
                    break;
                }
            }
        }
    }
    fn queue_replies(&mut self, replies: Vec<u8>) {
        if self.write_error.is_some() {
            self.dropped_reply_bytes = self.dropped_reply_bytes.saturating_add(replies.len());
        } else if self.replies.len() + replies.len() > READ_BUDGET {
            self.dropped_reply_bytes = self.dropped_reply_bytes.saturating_add(replies.len());
            self.fail_writer("pending reply byte limit".into());
        } else {
            self.replies.extend(replies);
        }
    }
    pub fn take_completed_sync_frame(&mut self) -> Option<CellFrame> {
        self.terminal.completed_sync_frame.take()
    }
    pub fn poll(&mut self) -> Result<usize, String> {
        self.poll_with_sync_capture(false)
    }
    /// Opt-in bounded snapshot at first completed sync block. Parsing never stops.
    pub fn poll_with_sync_capture(&mut self, capture: bool) -> Result<usize, String> {
        self.poll_output(capture, !self.holding_replies)
    }

    /// Parse/drain with the ordinary budgets, but hold protocol writes until
    /// the sole bridge has confirmed the active logical frame boundary.
    #[doc(hidden)]
    pub fn poll_holding_replies(&mut self, capture: bool) -> Result<usize, String> {
        self.holding_replies = self.write_error.is_none();
        self.poll_output(capture, false)
    }

    /// Release deferred replies after a known event boundary, never input.
    #[doc(hidden)]
    pub fn release_held_replies(&mut self) -> bool {
        self.flush_replies();
        if self.replies.is_empty() {
            self.holding_replies = false;
            true
        } else {
            false
        }
    }

    /// Retire write authority without closing the child or its readable tail.
    #[doc(hidden)]
    pub fn fail_input_boundary(&mut self, reason: &'static str) {
        self.fail_writer(reason.into());
    }

    fn poll_output(&mut self, capture: bool, flush_replies: bool) -> Result<usize, String> {
        self.terminal.completed_sync_frame = None;
        self.terminal.capture_sync_end = capture;
        if self.closed {
            return Err("session is closed".into());
        }
        // A reply failure must never prevent draining the PTY's final output.
        if flush_replies {
            self.flush_replies();
        }
        let mut total = 0;
        for _ in 0..64 {
            if self.read_eof || total >= READ_BUDGET {
                break;
            }
            let mut bytes = [0; 4096];
            let remaining = (READ_BUDGET - total).min(bytes.len());
            match self
                .reader
                .as_mut()
                .ok_or("reader closed")?
                .read(&mut bytes[..remaining])
            {
                Ok(0) => {
                    self.read_eof = true;
                    break;
                }
                Ok(n) => {
                    total += n;
                    let replies = self.terminal.feed(&bytes[..n])?;
                    self.queue_replies(replies);
                    if flush_replies {
                        self.flush_replies();
                    }
                }
                Err(e) if e.raw_os_error() == Some(libc::EIO) => {
                    // Linux PTY master reports EIO after the final slave closes.
                    self.read_eof = true;
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        if !self.exited {
            match self.child.as_mut().ok_or("child transferred")?.try_wait() {
                Ok(status) => self.exited = status.is_some(),
                Err(error) => {
                    // A wait error before handoff also permanently revokes
                    // signalling authority; later close is observe-only.
                    self.pidfd.take();
                    return Err(error.to_string());
                }
            }
        }
        if self.read_eof && !self.replies.is_empty() {
            self.fail_writer("PTY ended before replies were delivered".into());
        }
        Ok(total)
    }
    pub fn resize(&mut self, rows: usize, cols: usize) -> Result<(), String> {
        if self.closed {
            return Err("session is closed".into());
        }
        let master = self.master.as_ref().ok_or("master closed")?;
        self.terminal.resize_with(rows, cols, || {
            master
                .resize(PtySize {
                    rows: rows as u16,
                    cols: cols as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| e.to_string())
        })
    }
    /// Drop PTY handles and transfer owned child to the fixed-capacity reaper.
    /// Success means accepted for asynchronous cleanup, not confirmed exit.
    pub fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.dropped_reply_bytes = self.dropped_reply_bytes.saturating_add(self.replies.len());
        self.replies.clear();
        self.writer.take();
        self.reader.take();
        self.master.take();
        if let (Some(child), Some(lease)) = (self.child.take(), self.lease.take()) {
            lease.handoff(child, self.pidfd.take());
        }
        Ok(())
    }
}
#[cfg(target_os = "linux")]
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    fn await_cleanup(s: &Session) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while s.cleanup_pending() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!s.cleanup_pending(), "owned child did not finish cleanup");
    }
    struct ScriptedWriter {
        steps: VecDeque<Result<usize, std::io::ErrorKind>>,
        bytes: Vec<u8>,
    }
    impl Write for ScriptedWriter {
        fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
            match self.steps.pop_front().unwrap_or(Ok(input.len())) {
                Ok(n) => {
                    let n = n.min(input.len());
                    self.bytes.extend(&input[..n]);
                    Ok(n)
                }
                Err(kind) => Err(kind.into()),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn interrupted_and_partial_write_preserve_suffix() {
        use std::io::ErrorKind::{Interrupted, WouldBlock};
        let mut writer = ScriptedWriter {
            steps: [Err(Interrupted), Ok(2), Err(WouldBlock), Ok(2)].into(),
            bytes: vec![],
        };
        assert_eq!(bounded_write(&mut writer, b"abcd").unwrap(), 2);
        assert_eq!(bounded_write(&mut writer, b"cd").unwrap(), 0);
        assert_eq!(bounded_write(&mut writer, b"cd").unwrap(), 2);
        assert_eq!(writer.bytes, b"abcd");
        writer.steps = [Err(Interrupted); 8].into();
        assert_eq!(bounded_write(&mut writer, b"e").unwrap(), 0);
        writer.steps = [Ok(0)].into();
        assert_eq!(
            bounded_write(&mut writer, b"e").unwrap_err().kind(),
            std::io::ErrorKind::WriteZero
        );
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn held_reply_waits_for_a_confirmed_continuation_before_ordinary_input() {
        use std::io::{Cursor, ErrorKind};
        use std::sync::Mutex;

        struct Available(Cursor<&'static [u8]>);
        impl Read for Available {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(bytes)?;
                if n == 0 {
                    return Err(ErrorKind::WouldBlock.into());
                }
                Ok(n)
            }
        }
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut session = Session::spawn(4, 80).unwrap();
        let wire = Arc::new(Mutex::new(Vec::new()));
        session.writer = Some(Box::new(Capture(wire.clone())));
        session.reader = Some(Box::new(Available(Cursor::new(b"\x1b[6n"))));
        let prefix = std::num::NonZeroU16::new(6).unwrap();
        let tail = b"OWNED\x1b[201~";
        assert_eq!(session.write(b"\x1b[200~").unwrap(), 6);
        assert!(session
            .continue_partial_input(tail, prefix, tail.len())
            .is_err());
        assert_eq!(session.poll_holding_replies(false).unwrap(), 4);
        assert_eq!(session.write(b"FRESH").unwrap(), 0);
        assert_eq!(&*wire.lock().unwrap(), b"\x1b[200~");
        assert!(session
            .continue_partial_input(&tail[..1], prefix, tail.len())
            .is_err());
        assert_eq!(
            session
                .continue_partial_input(tail, prefix, tail.len())
                .unwrap(),
            tail.len()
        );
        assert!(session.release_held_replies());
        assert_eq!(session.write(b"FRESH").unwrap(), 5);
        assert_eq!(
            &*wire.lock().unwrap(),
            b"\x1b[200~OWNED\x1b[201~\x1b[1;1RFRESH"
        );
        session.close().unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn broken_reply_writer_does_not_hide_output_tail() {
        let mut session = Session::spawn(4, 80).unwrap();
        session.writer = Some(Box::new(ScriptedWriter {
            steps: [Ok(1), Err(std::io::ErrorKind::BrokenPipe)].into(),
            bytes: vec![],
        }));
        session.replies.extend(b"reply");
        session.reader = Some(Box::new(std::io::Cursor::new(b"TAIL_AFTER_REPLY_FAILURE")));
        assert_eq!(session.poll().unwrap(), 24);
        assert!(session
            .terminal()
            .visible_text()
            .contains("TAIL_AFTER_REPLY_FAILURE"));
        assert!(session.write_error().is_some());
        assert_eq!(session.dropped_reply_bytes(), 4);
        assert!(session.replies.is_empty());
        assert!(session.write(b"x").is_err());
        assert_eq!(session.poll().unwrap(), 0);
        session.close().unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn exit_is_not_drained_until_budgeted_tail_reaches_eof() {
        let mut session = Session::spawn(4, 80).unwrap();
        let mut bytes = vec![b'x'; READ_BUDGET];
        bytes.extend_from_slice(b"\r\nFINAL_TAIL");
        session.reader = Some(Box::new(std::io::Cursor::new(bytes)));
        session.exited = true; // Deterministic ordering: exit observed before final reads.
        assert!(!session.is_drained());
        assert_eq!(session.poll().unwrap(), READ_BUDGET);
        assert!(!session.is_drained());
        assert_eq!(session.poll().unwrap(), 12);
        assert!(session.is_drained());
        assert!(session.terminal().visible_text().contains("FINAL_TAIL"));
        session.close().unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn reply_overflow_fails_only_writer_and_counts_discard() {
        let mut session = Session::spawn(4, 80).unwrap();
        session.queue_replies(vec![b'a'; READ_BUDGET]);
        session.queue_replies(vec![b'b'; 1]);
        assert_eq!(session.write_error(), Some("pending reply byte limit"));
        assert_eq!(session.dropped_reply_bytes(), READ_BUDGET + 1);
        session.queue_replies(vec![b'c'; 2]);
        assert_eq!(session.dropped_reply_bytes(), READ_BUDGET + 3);
        assert!(session.replies.is_empty());
        session.close().unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn interrupted_reads_are_bounded_and_eio_marks_eof() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct InterruptedReader(Arc<AtomicUsize>);
        impl Read for InterruptedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(std::io::ErrorKind::Interrupted.into())
            }
        }
        struct EndedReader;
        impl Read for EndedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(libc::EIO))
            }
        }
        let mut session = Session::spawn(4, 80).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        session.reader = Some(Box::new(InterruptedReader(calls.clone())));
        assert_eq!(session.poll().unwrap(), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 64);
        assert!(!session.read_eof);
        session.reader = Some(Box::new(EndedReader));
        assert_eq!(session.poll().unwrap(), 0);
        assert!(session.read_eof);
        session.close().unwrap();
    }
    #[test]
    fn row_effects_are_already_applied_exactly_once() {
        let mut t = Terminal::new(3, 12).unwrap();
        t.feed(b"abcdef\r\x1b[2C\x1b[2P").unwrap();
        assert!(t.visible_text().starts_with("abef"));
        t.feed(b"\x1b[2@XY").unwrap();
        assert!(t.visible_text().starts_with("abXYef"));
        t.feed(b"\x1b[2X").unwrap();
        assert!(t.visible_text().starts_with("abXY  "));
        t.feed(b"\r\x1b[2K").unwrap();
        assert!(t.visible_text().lines().next().unwrap().trim().is_empty());
    }
    #[test]
    fn alternate_saved_slots_repeated_entry_and_resize() {
        let mut t = Terminal::new(4, 12).unwrap();
        t.feed(b"PRIMARY\x1b[31m\x1b[?1049hALT\x1b[?1048hX\x1b[?1048lY\x1b[?1049h")
            .unwrap();
        assert!(t.screen.use_alt_buffer);
        assert!(t.visible_text().starts_with("ALTY"));
        assert_eq!(t.frame().unwrap().stamp().screen, Screen::Alternate);
        t.resize(5, 14).unwrap();
        t.feed(b"\x1b[?7l\x1b[?1049lZ").unwrap();
        assert!(!t.screen.use_alt_buffer);
        assert!(t.visible_text().starts_with("PRIMARYZ"));
        assert!(t.protocol.modes().contains(&7));
        assert_eq!(t.screen.current_fg, jterm_engine::Color::Red);
        assert_eq!(t.unsupported_count(), 0);
    }
    #[test]
    fn mode47_retains_alternate_1047_clears_and_keyboard_slots_swap_once() {
        let mut t = Terminal::new(4, 12).unwrap();
        t.feed(b"\x1b[=2u\x1b[?47hA\x1b[=4u\x1b[?47h").unwrap();
        assert_eq!(t.protocol.keyboard_enhancement_flags(), 4);
        t.feed(b"\x1b[?47l").unwrap();
        assert_eq!(t.protocol.keyboard_enhancement_flags(), 2);
        t.feed(b"\x1b[?47h").unwrap();
        assert!(t.visible_text().starts_with('A'));
        t.feed(b"\x1b[?47l\x1b[?1047h").unwrap();
        assert!(t.visible_text().trim().is_empty());
        assert_eq!(t.screen.cursor_col, 0);
    }
    #[test]
    fn erase_and_region_edits_use_shared_screen() {
        let mut t = Terminal::new(4, 8).unwrap();
        t.feed(b"111\r\n222\r\n333\r\n444\x1b[2;1H\x1b[L").unwrap();
        assert!(t.visible_text().lines().nth(1).unwrap().trim().is_empty());
        assert!(t.visible_text().lines().nth(2).unwrap().starts_with("222"));
        t.feed(b"\x1b[M").unwrap();
        assert!(t.visible_text().lines().nth(1).unwrap().starts_with("222"));
        t.feed(b"\x1b[2;3r\x1b[S").unwrap();
        assert!(t.visible_text().lines().next().unwrap().starts_with("111"));
        assert!(t.visible_text().lines().nth(1).unwrap().starts_with("333"));
        t.feed(b"\x1b[2J").unwrap();
        assert!(t.visible_text().trim().is_empty());
        assert_eq!(t.unsupported_count(), 0);
    }
    #[test]
    fn host_sequences_fragment_identically_and_unknown_modes_stay_visible() {
        let stream = b"PRIMARY\x1b[?1049h\x1b[2J\x1b[2;2HZ\x1b[3b\x1bM\x1bE\x1bD\x1b[?25l\x1b[?1049l\x1b[?9999h";
        let mut a = Terminal::new(4, 16).unwrap();
        let mut b = Terminal::new(4, 16).unwrap();
        a.feed(stream).unwrap();
        for byte in stream {
            b.feed(&[*byte]).unwrap();
        }
        assert_eq!(a.visible_text(), b.visible_text());
        assert_eq!(a.screen.cursor_col, b.screen.cursor_col);
        assert!(!a.frame().unwrap().cursor().visible);
        assert_eq!(a.unsupported_count(), 1);
    }
    #[test]
    fn unsupported_diagnostics_are_bounded_and_omit_payload() {
        let mut t = Terminal::new(3, 10).unwrap();
        t.feed(b"\x1b]52;c;PRIVATE_CLIPBOARD\x07").unwrap();
        for mode in 3000..3050 {
            t.feed(format!("\x1b[?{mode}h").as_bytes()).unwrap();
        }
        assert_eq!(t.unsupported_events().len(), 16);
        assert!(t
            .unsupported_events()
            .iter()
            .all(|s| s.len() < 128 && !s.contains("PRIVATE")));
        assert_eq!(t.unsupported_count(), 51);
    }
    #[test]
    fn keyboard_mode_modifiers_and_unicode_are_rust_owned() {
        let mut t = Terminal::new(3, 10).unwrap();
        assert_eq!(
            t.encode_key("ArrowUp", false, false, false).unwrap(),
            b"\x1b[A"
        );
        t.feed(b"\x1b[?1h").unwrap();
        assert_eq!(
            t.encode_key("ArrowUp", false, false, false).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            t.encode_key("ArrowUp", true, false, true).unwrap(),
            b"\x1b[1;6A"
        );
        assert_eq!(
            t.encode_key("界", false, true, false).unwrap(),
            "\x1b界".as_bytes()
        );
        assert_eq!(t.encode_key("c", false, false, true).unwrap(), [3]);
        assert_eq!(t.encode_key("[", false, false, true).unwrap(), [27]);
        assert_eq!(t.encode_key("Tab", true, false, false).unwrap(), b"\x1b[Z");
        assert_eq!(t.encode_key("Enter", false, false, false).unwrap(), b"\r");
        assert!(t.encode_key("界", false, false, true).is_err());
        assert!(t.encode_key("\x1b[2J", false, false, false).is_err());
        assert!(t.encode_key("Dead", false, false, false).is_err());
        t.feed(b"\x1b[=1u").unwrap();
        assert!(t.encode_key("a", false, false, false).is_err());
    }
    #[test]
    fn paste_controls_markers_normalization_and_wire_bound() {
        let mut t = Terminal::new(3, 10).unwrap();
        assert_eq!(
            t.encode_paste("a\r\nb\nc\rd\t界").unwrap(),
            "a\rb\rc\rd\t界".as_bytes()
        );
        for text in ["\x1b[201~", "a\0b", "\u{0085}", "\x7f"] {
            assert!(t.encode_paste(text).is_err());
        }
        assert!(t.encode_paste(&"x".repeat(4096)).is_ok());
        assert!(t.encode_paste(&"x".repeat(4097)).is_err());
        t.feed(b"\x1b[?2004h").unwrap();
        assert_eq!(t.encode_paste("a\nb").unwrap(), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(t.encode_paste(&"x".repeat(4084)).unwrap().len(), 4096);
        assert!(t.encode_paste(&"x".repeat(4085)).is_err());
        t.feed(b"\x1b[?2004l").unwrap();
        assert_eq!(t.encode_paste("x").unwrap(), b"x");
        assert_eq!(t.unsupported_count(), 0);
    }
    #[test]
    fn vim_modify_other_keys_does_not_block_plain_escape_or_function_keys() {
        let mut t = Terminal::new(3, 10).unwrap();
        t.feed(b"\x1b[>4;2m").unwrap();
        assert_eq!(t.encode_key("Escape", false, false, false).unwrap(), [27]);
        assert_eq!(t.encode_key("Enter", false, false, false).unwrap(), [13]);
        assert_eq!(
            t.encode_key("ArrowUp", false, false, false).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(t.encode_key("A", true, false, false).unwrap(), b"A");
        assert_eq!(
            t.encode_key("c", false, false, true).unwrap(),
            b"\x1b[27;5;99~"
        );
        assert_eq!(
            t.encode_key("Escape", false, false, true).unwrap(),
            b"\x1b[27;5;27~"
        );
        t.feed(b"\x1b[>4;1f").unwrap();
        assert_eq!(
            t.encode_key("c", false, false, true).unwrap(),
            b"\x1b[99;5u"
        );
        t.feed(b"\x1b[>4;1m").unwrap();
        assert_eq!(t.encode_key("c", false, false, true).unwrap(), [3]);
        assert_eq!(
            t.encode_key("c", false, true, false).unwrap(),
            b"\x1b[99;3u"
        );
        t.feed(b"\x1b[>4;0m").unwrap();
        assert_eq!(t.encode_key("c", false, true, false).unwrap(), b"\x1bc");
    }
    #[test]
    fn committed_text_preserves_multiscalar_utf8_without_key_or_paste_wrapping() {
        let mut t = Terminal::new(3, 10).unwrap();
        let text = "中文e\u{301}👨\u{200d}👩";
        assert_eq!(t.encode_text(text).unwrap(), text.as_bytes());
        // Bracketed paste and modified-key protocols must not reinterpret IME.
        t.feed(b"\x1b[?2004h\x1b[>4;2m\x1b[>4;1f").unwrap();
        assert_eq!(t.encode_text(text).unwrap(), text.as_bytes());
        assert_eq!(t.encode_text("A").unwrap(), b"A");
        assert!(t.encode_text("").unwrap().is_empty());
        t.feed(b"\x1b[=8u").unwrap();
        assert!(t.encode_text(text).is_err());
        assert!(t.encode_text("").unwrap().is_empty());
        t.feed(b"\x1b[=0u").unwrap();
        assert_eq!(t.encode_text(text).unwrap(), text.as_bytes());
    }
    #[test]
    fn committed_text_rejects_controls_and_enforces_utf8_bytes_not_scalars() {
        let t = Terminal::new(3, 10).unwrap();
        for text in ["a\nb", "\r", "\t", "\0", "\x1b[201~", "\x7f", "\u{0085}"] {
            assert!(t.encode_text(text).is_err());
        }
        assert_eq!(t.encode_text(&"é".repeat(2048)).unwrap().len(), 4096);
        assert!(t.encode_text(&"é".repeat(2049)).is_err());
        assert!(t.encode_text(&"x".repeat(4097)).is_err());
    }
    #[test]
    fn xtgettcap_negative_reply_does_not_claim_or_reflect_capabilities() {
        let mut t = Terminal::new(3, 10).unwrap();
        assert_eq!(
            t.feed(b"\x1bP+q544e;436f\x1b\\").unwrap(),
            b"\x1bP0+r\x1b\\"
        );
        assert_eq!(t.negative_query_replies(), 1);
        assert_eq!(t.unsupported_count(), 1);
        assert_eq!(t.unsupported_events(), &["DCS XTGETTCAP"]);
        assert!(t.feed(b"\x1b[>c").unwrap().is_empty()); // No fabricated DA2 identity.
        let before = t.negative_query_replies();
        assert!(t.feed(b"\x1b]52;c;SECRET\x07").unwrap().is_empty());
        assert_eq!(t.negative_query_replies(), before);
        let mut fragmented = Terminal::new(3, 10).unwrap();
        let mut replies = Vec::new();
        for byte in b"\x1bP+qnot-hex\x1b\\" {
            replies.extend(fragmented.feed(&[*byte]).unwrap());
        }
        assert_eq!(replies, b"\x1bP0+r\x1b\\");
    }
    #[test]
    fn vim_dcs_probe_is_opaque_without_reply_or_cursor_mutation() {
        let mut t = Terminal::new(3, 10).unwrap();
        t.feed(b"AB").unwrap();
        let before = t.visible_text();
        assert!(t.feed(b"\x1bPzz\x1b\\").unwrap().is_empty());
        assert_eq!(t.visible_text(), before);
        assert_eq!(t.screen.cursor_col, 2);
        assert_eq!(t.unsupported_events(), &["DCS Vim compatibility probe"]);
        assert!(t
            .feed(b"\x1bPunknown_PRIVATE_payload\x1b\\")
            .unwrap()
            .is_empty());
        assert!(t
            .unsupported_events()
            .iter()
            .all(|s| !s.contains("PRIVATE") && !s.contains("prefix=")));
        assert_eq!(t.negative_query_replies(), 0);
    }
    #[test]
    fn vim_unknown_dcs_and_intermediate_sgr_probe_preserves_style_and_cpr() {
        let mut t = Terminal::new(3, 10).unwrap();
        t.feed(b"\x1b[1;31mAB").unwrap();
        let before = t.visible_text();
        let probe = b"\x1bPzz\x1b\\\x1b[0%m\x1b[6n";
        assert_eq!(t.feed(probe).unwrap(), b"\x1b[1;3R");
        assert_eq!(t.visible_text(), before);
        assert_eq!(t.screen.current_fg, jterm_engine::Color::Red);
        assert!(t.screen.current_flags.bold());
        assert_eq!(t.unsupported_count(), 2);
        assert_eq!(t.negative_query_replies(), 0);
        let mut b = Terminal::new(3, 10).unwrap();
        b.feed(b"\x1b[1;31mAB").unwrap();
        let mut replies = Vec::new();
        for byte in probe {
            replies.extend(b.feed(&[*byte]).unwrap());
        }
        assert_eq!(replies, b"\x1b[1;3R");
        assert_eq!(b.screen.current_fg, jterm_engine::Color::Red);
        assert!(b.screen.current_flags.bold());
        assert_eq!(b.visible_text(), before);
    }
    #[test]
    fn primary_history_captures_scroll_and_snapshot_is_immutable() {
        let mut t = Terminal::new(2, 8).unwrap();
        t.feed(b"one\r\ntwo\r\nthree\r\n").unwrap();
        assert_eq!(t.history_rows(), 2);
        let page = t.history_page(0, 128).unwrap();
        assert_eq!(page.text, "one\ntwo");
        assert_eq!(page.selection_text(0, 1, 1, 2).unwrap(), "ne\ntw");
        assert_eq!(page.selection_text(0, 0, 1, 0).unwrap(), "one\n");
        assert!(page.selection_text(0, 9, 1, 0).is_err());
        assert!(page.selection_text(1, 0, 0, 0).is_err());
        assert!(page.selection_text(0, 0, 3, 0).is_err());
        t.feed(b"four\r\n").unwrap();
        assert_eq!(page.text, "one\ntwo");
        assert_eq!(t.history_page(1, 1).unwrap().text, "two");
        assert_eq!(t.history_page(t.history_rows(), 1).unwrap().row_count, 0);
    }
    #[test]
    fn alternate_and_partial_margins_never_archive_or_expose_primary_history() {
        let mut t = Terminal::new(3, 8).unwrap();
        t.feed(b"a\r\nb\r\nc\r\n").unwrap();
        assert_eq!(t.history_rows(), 1);
        t.feed(b"\x1b[?1049hALT\r\nX\r\nY\r\nZ\x1b[3J").unwrap();
        assert_eq!(t.history_rows(), 0);
        assert!(t.history_page(0, 128).unwrap().text.is_empty());
        assert_eq!(t.history_page(1, 128).unwrap().offset, 0);
        t.feed(b"\x1b[?1049l").unwrap();
        assert_eq!(t.history_page(0, 128).unwrap().text, "a");
        t.feed(b"\x1b[2;3r\x1b[3;1H\n\n\x1b[S").unwrap();
        assert_eq!(t.history_rows(), 1);
        t.feed(b"\x1b[3J").unwrap();
        assert_eq!(t.history_rows(), 0);
    }
    #[test]
    fn history_eviction_and_page_limits_are_bounded() {
        let mut t = Terminal::new(1, 8).unwrap();
        for n in 0..1030 {
            t.feed(format!("{n}\r\n").as_bytes()).unwrap();
        }
        assert_eq!(t.history_rows(), MAX_HISTORY_ROWS);
        assert_eq!(t.history_page(1023, 1).unwrap().text, "6");
        assert_eq!(t.history_page(0, 1).unwrap().text, "1029");
        assert!(t.history_page(0, 0).is_err());
        assert!(t.history_page(0, 129).is_err());
        assert!(t.history_page(1025, 1).is_err());
    }
    #[test]
    fn history_wide_softwrap_and_resize_keep_original_geometry() {
        let mut t = Terminal::new(1, 4).unwrap();
        t.feed("界abXY\r\n".as_bytes()).unwrap();
        let page = t.history_page(0, 128).unwrap();
        assert_eq!(page.text, "界abXY");
        assert_eq!(page.selection_text(0, 0, 0, 2).unwrap(), "界");
        assert_eq!(page.selection_text(0, 1, 0, 2).unwrap(), "");
        t.resize(2, 2).unwrap();
        assert_eq!(t.history_page(0, 128).unwrap().text, "界abXY");
        assert_eq!(t.history_rows(), 2);
        t.feed(b"\x1b[S").unwrap();
        assert_eq!(t.history_rows(), 3);
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn fixed_shell_real_pty_archives_scrolled_output() {
        let mut s = Session::spawn(2, 80).unwrap();
        let command = b"printf 'HISTORY_A\\nHISTORY_B\\nHISTORY_C\\nHISTORY_D\\n'; exit\n";
        let mut sent = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !s.is_drained() && std::time::Instant::now() < deadline {
            if sent < command.len() && !s.is_exited() {
                sent += s.write(&command[sent..]).unwrap();
            }
            s.poll().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(s.is_drained());
        let page = s.terminal().history_page(0, 128).unwrap();
        assert!(
            page.text.lines().any(|line| line == "HISTORY_B"),
            "{}",
            page.text
        );
        assert!(page.row_count > 0);
        assert_eq!(
            s.terminal().history_page(1024, 128).unwrap().offset,
            page.total_rows
        );
        assert!(s.terminal().history_page(usize::MAX, 128).is_err());
        s.close().unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn short_reads_cannot_overshoot_poll_byte_budget() {
        struct ShortFirst {
            first: bool,
            bytes: std::io::Cursor<Vec<u8>>,
        }
        impl Read for ShortFirst {
            fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
                if self.first {
                    self.first = false;
                    self.bytes.read(&mut target[..1])
                } else {
                    self.bytes.read(target)
                }
            }
        }
        let mut s = Session::spawn(2, 80).unwrap();
        s.reader = Some(Box::new(ShortFirst {
            first: true,
            bytes: std::io::Cursor::new(vec![b'x'; READ_BUDGET + 1]),
        }));
        assert_eq!(s.poll().unwrap(), READ_BUDGET);
        assert!(!s.read_eof);
        assert_eq!(s.poll().unwrap(), 1);
        assert!(s.read_eof);
        s.close().unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn real_pty_large_exit_tail_is_drained_without_budget_overrun() {
        for _ in 0..3 {
            let mut s = Session::spawn(3, 80).unwrap();
            let command = b"printf '%070000d\\nEXIT_TAIL_VERIFIED\\n' 0; exit\n";
            let mut sent = 0;
            let mut received = 0;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !s.is_drained() && std::time::Instant::now() < deadline {
                if !s.is_exited() && sent < command.len() {
                    sent += s.write(&command[sent..]).unwrap();
                }
                let n = s.poll().unwrap();
                assert!(n <= READ_BUDGET);
                received += n;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert!(s.is_drained());
            assert!(received > 70000);
            assert!(s
                .terminal()
                .visible_text()
                .lines()
                .any(|line| line.trim_end() == "EXIT_TAIL_VERIFIED"));
            assert!(s.write(b"x").is_err());
            s.close().unwrap();
            s.close().unwrap();
            assert!(s.poll().is_err());
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn real_pty_repeated_live_close_is_idempotent() {
        for _ in 0..8 {
            let mut s = Session::spawn(2, 80).unwrap();
            s.close().unwrap();
            s.close().unwrap();
            await_cleanup(&s);
            assert!(s.is_exited());
            assert!(s.master.is_none() && s.reader.is_none() && s.writer.is_none());
            assert!(s.resize(3, 90).is_err());
            assert!(s.write(b"x").is_err());
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn poll_wait_failure_handoff_is_observe_only_and_reaped() {
        #[derive(Debug)]
        struct FailOnceChild {
            inner: Option<Box<dyn Child + Send + Sync>>,
            fail: bool,
        }
        impl portable_pty::ChildKiller for FailOnceChild {
            fn kill(&mut self) -> std::io::Result<()> {
                self.inner.as_mut().unwrap().kill()
            }
            fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
                self.inner.as_ref().unwrap().clone_killer()
            }
        }
        impl Child for FailOnceChild {
            fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
                if self.fail {
                    self.fail = false;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                self.inner.as_mut().unwrap().try_wait()
            }
            fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
                self.inner.as_mut().unwrap().wait()
            }
            fn process_id(&self) -> Option<u32> {
                self.inner.as_ref().unwrap().process_id()
            }
        }
        let mut s = Session::spawn(2, 80).unwrap();
        let child = s.child.take().unwrap();
        s.child = Some(Box::new(FailOnceChild {
            inner: Some(child),
            fail: true,
        }));
        assert!(s.poll().is_err());
        assert!(s.pidfd.is_none());
        s.close().unwrap();
        assert!(s.closed);
        assert!(s.writer.is_none() && s.reader.is_none() && s.master.is_none());
        s.close().unwrap();
        await_cleanup(&s);
        assert!(s.is_exited());
        s.close().unwrap();
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn real_pty_hup_ignoring_owned_shell_is_reaped_via_pidfd() {
        let mut s = Session::spawn(4, 100).unwrap();
        let command = b"trap '' HUP; printf '%s%s\\n' PIDFD_ READY; while :; do :; done\n";
        let mut sent = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut ready = false;
        while std::time::Instant::now() < deadline {
            if sent < command.len() {
                sent += s.write(&command[sent..]).unwrap();
            }
            s.poll().unwrap();
            ready = s
                .terminal()
                .visible_text()
                .lines()
                .any(|line| line.trim_end().ends_with("PIDFD_READY"));
            if ready {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(ready, "fixed shell did not install HUP trap");
        assert!(!s.is_exited());
        s.close().unwrap();
        await_cleanup(&s);
        assert!(s.is_exited());
        s.close().unwrap();
    }
    #[test]
    fn shared_parser_fragmentation_and_sgr() {
        let mut a = Terminal::new(4, 40).unwrap();
        let mut b = Terminal::new(4, 40).unwrap();
        let bytes = "hello\r\n\x1b[31m世界\x1b[0m!".as_bytes();
        a.feed(bytes).unwrap();
        for byte in bytes {
            b.feed(&[*byte]).unwrap();
        }
        assert_eq!(a.visible_text(), b.visible_text());
        assert_eq!(a.screen.grid[1][0].foreground, jterm_engine::Color::Red);
        assert_eq!(a.screen.grid[1][0].character, '世');
    }
    #[test]
    fn utf8_every_split_preserves_wide_frame_and_colors() {
        let bytes = "a界中\x1b[31m文\x1b[0mZ".as_bytes();
        let mut baseline = Terminal::new(3, 5).unwrap();
        baseline.feed(bytes).unwrap();
        for cut in 0..=bytes.len() {
            let mut t = Terminal::new(3, 5).unwrap();
            t.feed(&bytes[..cut]).unwrap();
            t.feed(&bytes[cut..]).unwrap();
            for row in 0..3 {
                assert_eq!(t.screen.grid[row], baseline.screen.grid[row], "cut={cut}");
            }
            assert_eq!(
                t.frame().unwrap().cursor(),
                baseline.frame().unwrap().cursor()
            );
            assert_eq!(t.unsupported_count(), 0);
        }
    }
    #[test]
    fn wide_overwrite_repairs_both_halves_and_wraps() {
        let mut t = Terminal::new(3, 4).unwrap();
        t.feed("界中".as_bytes()).unwrap();
        assert!(t.screen.grid[0][0].flags.wide());
        assert!(t.screen.grid[0][1].flags.wide_continuation());
        t.feed(b"X").unwrap();
        assert_eq!(t.screen.grid[1][0].character, 'X');
        t.feed(b"\x1b[1;2HA").unwrap();
        assert_eq!(t.screen.grid[0][0].character, ' ');
        assert_eq!(t.screen.grid[0][1].character, 'A');
        assert!(!t.screen.grid[0][1].flags.wide_continuation());
        t.feed(b"\x1b[1;3HB").unwrap();
        assert_eq!(t.screen.grid[0][3].character, ' ');
        assert!(!t.screen.grid[0][3].flags.wide_continuation());
        t.feed("\x1b[2;4H界".as_bytes()).unwrap();
        assert_eq!(t.screen.grid[2][0].character, '界');
        assert!(t.screen.grid[2][1].flags.wide_continuation());
        t.feed("\x1b[?7l\x1b[2;4H中".as_bytes()).unwrap();
        assert_eq!(t.screen.grid[1][2].character, '中');
        assert!(t.screen.grid[1][3].flags.wide_continuation());
        t.frame().unwrap();
    }
    #[test]
    fn narrow_resize_repairs_pairs_and_single_column_retains_core_truncated_lead() {
        let mut t = Terminal::new(2, 4).unwrap();
        t.feed("ab界".as_bytes()).unwrap();
        t.resize(2, 3).unwrap();
        assert_eq!(t.screen.grid[0][2].character, ' ');
        assert!(!t.screen.grid[0][2].flags.wide());
        t.resize(2, 1).unwrap();
        t.feed("\x1b[H界".as_bytes()).unwrap();
        assert_eq!(t.screen.grid[1][0].character, '界');
        assert!(t.screen.grid[1][0].flags.wide());
        assert!(!t.screen.grid[1][0].flags.wide_continuation());
        assert_eq!(t.unsupported_count(), 0);
        t.frame().unwrap();
    }
    #[test]
    fn combining_marks_are_explicitly_unsupported_without_cursor_or_base_damage() {
        let mut t = Terminal::new(2, 4).unwrap();
        t.feed("e\u{301}\u{200d}X".as_bytes()).unwrap();
        assert_eq!(t.screen.grid[0][0].character, 'e');
        assert_eq!(t.screen.grid[0][1].character, 'X');
        assert_eq!(t.screen.cursor_col, 2);
        assert_eq!(t.unsupported_count(), 2);
        assert_eq!(
            t.unsupported_events(),
            &["zero-width scalar / grapheme composition unsupported"]
        );
    }
    // Feed the production parser rather than calling UnicodeWidthChar directly:
    // FF9E/FF9F changed from zero to one cell in unicode-width 0.2.3. These
    // assertions cover host geometry, not WebView glyph shaping or IME input.
    #[test]
    fn unicode_023_halfwidth_marks_keep_four_cells_at_every_utf8_split() {
        let input = "ﾊﾟｸﾞ".as_bytes();
        for cut in 0..=input.len() {
            let mut t = Terminal::new(2, 8).unwrap();
            t.feed(&input[..cut]).unwrap();
            t.feed(&input[cut..]).unwrap();
            for (col, character) in ['ﾊ', 'ﾟ', 'ｸ', 'ﾞ'].into_iter().enumerate() {
                let cell = &t.screen.grid[0][col];
                assert_eq!(cell.character, character, "cut={cut}, col={col}");
                assert!(!cell.flags.wide() && !cell.flags.wide_continuation());
            }
            let frame = t.frame().unwrap();
            assert_eq!((frame.cursor().row, frame.cursor().col), (0, 4));
            assert!(!frame.cursor().pending_wrap);
            assert_eq!(t.unsupported_count(), 0);
        }
    }
    #[test]
    fn unicode_023_halfwidth_marks_wrap_and_backspace_after_bytewise_feed() {
        let mut t = Terminal::new(3, 4).unwrap();
        for byte in "abcﾟ".as_bytes() {
            t.feed(&[*byte]).unwrap();
        }
        let frame = t.frame().unwrap();
        assert_eq!((frame.cursor().row, frame.cursor().col), (0, 3));
        assert!(frame.cursor().pending_wrap);
        assert_eq!(frame.rows()[0].cells[3].character, 'ﾟ');
        for byte in "ﾞX".as_bytes() {
            t.feed(&[*byte]).unwrap();
        }
        assert_eq!(t.screen.grid[1][0].character, 'ﾞ');
        assert_eq!(t.screen.grid[1][1].character, 'X');
        t.feed(b"\x08Y").unwrap();
        let frame = t.frame().unwrap();
        assert_eq!(frame.rows()[1].cells[1].character, 'Y');
        assert_eq!((frame.cursor().row, frame.cursor().col), (1, 2));
        assert!(!frame.cursor().pending_wrap);
        assert_eq!(t.unsupported_count(), 0);
    }
    #[test]
    fn unicode_023_zero_width_rejection_is_preserved_between_halfwidth_marks() {
        let input = "ﾟ\u{301}\u{200d}ﾞ".as_bytes();
        for cut in 0..=input.len() {
            let mut t = Terminal::new(2, 4).unwrap();
            t.feed(&input[..cut]).unwrap();
            t.feed(&input[cut..]).unwrap();
            assert_eq!(t.screen.grid[0][0].character, 'ﾟ', "cut={cut}");
            assert_eq!(t.screen.grid[0][1].character, 'ﾞ', "cut={cut}");
            assert_eq!(t.screen.cursor_col, 2);
            assert_eq!(t.unsupported_count(), 2);
            assert_eq!(
                t.unsupported_events(),
                &["zero-width scalar / grapheme composition unsupported"]
            );
            t.frame().unwrap();
        }
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn real_pty_chinese_wide_overwrite_and_wrap_frame() {
        let mut s = Session::spawn(4, 8).unwrap();
        // Fixed printf emits UTF-8 in separate writes; explicit cursor addressing
        // removes shell prompt and echo from the area under assertion.
        let command = b"printf '\\033[2J\\033[H'; printf '\\347'; printf '\\225\\214'; printf '\\033[1;2HA\\033[2;8H\\344\\270\\255'; exit\n";
        let mut sent = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !s.is_drained() && std::time::Instant::now() < deadline {
            if sent < command.len() && !s.is_exited() {
                sent += s.write(&command[sent..]).unwrap();
            }
            s.poll().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(s.is_drained());
        let frame = s.terminal().frame().unwrap();
        assert_eq!(frame.rows()[0].cells[0].character, ' ');
        assert_eq!(frame.rows()[0].cells[1].character, 'A');
        assert_eq!(frame.rows()[2].cells[0].character, '中');
        assert!(frame.rows()[2].cells[0].flags.wide());
        assert!(frame.rows()[2].cells[1].flags.wide_continuation());
        s.close().unwrap();
        await_cleanup(&s);
    }
    #[test]
    fn resize_validation_and_kernel_failure_are_atomic() {
        let mut t = Terminal::new(3, 8).unwrap();
        t.feed(b"abcdefgh").unwrap();
        let before = t.frame().unwrap();
        for (rows, cols) in [(0, 8), (3, 0), (129, 8), (3, 257), (usize::MAX, 1)] {
            assert!(t
                .resize_with(rows, cols, || panic!("invalid request reached PTY"))
                .is_err());
        }
        assert!(t
            .resize_with(1, 1, || Err("injected ioctl failure".into()))
            .is_err());
        let after = t.frame().unwrap();
        assert_eq!(after.stamp(), before.stamp());
        assert_eq!(after.cursor(), before.cursor());
        assert_eq!(after.cols(), before.cols());
        assert_eq!(after.rows(), before.rows());
        t.layout = u64::MAX;
        assert!(t
            .resize_with(1, 1, || panic!("overflow reached PTY"))
            .is_err());
        t.layout = 1;
        t.revision = u64::MAX;
        assert!(t
            .resize_with(1, 1, || panic!("overflow reached PTY"))
            .is_err());
    }
    #[test]
    fn cursor_shape_visibility_and_pending_wrap_across_resize_and_alternate() {
        use jterm_engine::CursorShape;
        let mut t = Terminal::new(3, 4).unwrap();
        for (param, shape) in [
            (0, CursorShape::Block),
            (1, CursorShape::Block),
            (2, CursorShape::Block),
            (3, CursorShape::Underline),
            (4, CursorShape::Underline),
            (5, CursorShape::Beam),
            (6, CursorShape::Beam),
        ] {
            t.feed(format!("\x1b[{param} q").as_bytes()).unwrap();
            assert_eq!(t.frame().unwrap().cursor().shape, shape);
        }
        t.feed(b"\x1b[?25labcd").unwrap();
        assert!(t.frame().unwrap().cursor().pending_wrap);
        assert!(!t.frame().unwrap().cursor().visible);
        t.resize(1, 1).unwrap();
        let frame = t.frame().unwrap();
        assert_eq!((frame.cursor().row, frame.cursor().col), (0, 0));
        assert!(!frame.cursor().pending_wrap);
        assert_eq!(frame.cursor().shape, CursorShape::Beam);
        assert!(!frame.cursor().visible);
        t.feed(b"\x1b[?1049h\x1b[4 q\x1b[?25h").unwrap();
        assert_eq!(t.frame().unwrap().cursor().shape, CursorShape::Underline);
        t.resize(128, 256).unwrap();
        t.feed(b"\x1b[999;999H").unwrap();
        assert_eq!(
            (
                t.frame().unwrap().cursor().row,
                t.frame().unwrap().cursor().col
            ),
            (127, 255)
        );
        t.feed(b"\x1b[?1049l").unwrap();
        assert_eq!(t.frame().unwrap().cursor().shape, CursorShape::Beam);
        // DECTCEM is global, not part of the saved primary cursor state.
        assert!(t.frame().unwrap().cursor().visible);
        assert_eq!(
            (
                t.frame().unwrap().cursor().row,
                t.frame().unwrap().cursor().col
            ),
            (0, 0)
        );
        assert_eq!(t.unsupported_count(), 0);
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn real_pty_resize_limits_match_kernel_and_reject_without_mutation() {
        let mut s = Session::spawn(4, 8).unwrap();
        for (rows, cols) in [(1, 1), (128, 256), (3, 9)] {
            s.resize(rows, cols).unwrap();
            let kernel = s.master.as_ref().unwrap().get_size().unwrap();
            assert_eq!((kernel.rows as usize, kernel.cols as usize), (rows, cols));
            let frame = s.terminal().frame().unwrap();
            assert_eq!((frame.rows().len(), frame.cols()), (rows, cols));
        }
        // Real slave output drives DECSCUSR, visibility and clamped positioning.
        let command = b"printf '\\033[6 q\\033[?25l\\033[999;999H'; exit\n";
        let mut sent = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !s.is_drained() && std::time::Instant::now() < deadline {
            if sent < command.len() && !s.is_exited() {
                sent += s.write(&command[sent..]).unwrap();
            }
            s.poll().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(s.is_drained());
        let before = s.terminal().frame().unwrap();
        assert_eq!(before.cursor().shape, jterm_engine::CursorShape::Beam);
        assert!(!before.cursor().visible);
        assert_eq!((before.cursor().row, before.cursor().col), (2, 8));
        for (rows, cols) in [(0, 1), (1, 0), (129, 1), (1, 257)] {
            assert!(s.resize(rows, cols).is_err());
        }
        let kernel = s.master.as_ref().unwrap().get_size().unwrap();
        assert_eq!((kernel.rows, kernel.cols), (3, 9));
        assert_eq!(s.terminal().frame().unwrap().stamp(), before.stamp());
        s.close().unwrap();
        assert!(s.resize(4, 8).is_err());
        assert_eq!(s.terminal().frame().unwrap().stamp(), before.stamp());
        await_cleanup(&s);
    }
    #[test]
    fn resize_and_unknown_are_explicit() {
        let mut t = Terminal::new(4, 20).unwrap();
        t.feed(b"ok\x1b[?9999h").unwrap();
        assert_eq!(t.unsupported_count(), 1);
        assert!(t.resize(0, 10).is_err());
        t.resize(5, 30).unwrap();
        let frame = t.frame().unwrap();
        assert_eq!(frame.cols(), 30);
        assert_eq!(frame.rows().len(), 5);
    }
    #[test]
    fn wrap_scroll_and_bounds() {
        let mut t = Terminal::new(2, 3).unwrap();
        t.feed(b"abcdefg").unwrap();
        assert_eq!(t.screen.grid[0][0].character, 'd');
        assert_eq!(t.screen.grid[1][0].character, 'g');
        assert!(t.feed(&vec![b'a'; READ_BUDGET + 1]).is_err());
        assert!(Terminal::new(200, 30).is_err());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn fixed_shell_real_pty_roundtrip_resize_and_close() {
        let mut s = Session::spawn(8, 80).unwrap();
        s.resize(10, 90).unwrap();
        let command = b"printf 'PTY_OK_314159\\n'; exit\n";
        let mut sent = 0;
        let start = std::time::Instant::now();
        while !s.is_drained() && start.elapsed() < std::time::Duration::from_secs(5) {
            if !s.is_exited() && sent < command.len() {
                sent += s.write(&command[sent..]).unwrap();
            }
            s.poll().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Drain output after the exit was observed.
        s.poll().unwrap();
        assert!(s.is_exited());
        assert!(s.is_drained());
        let text = s.terminal().visible_text();
        assert!(
            text.lines()
                .any(|l| l.trim_end().ends_with("PTY_OK_314159")),
            "{text}"
        );
        s.close().unwrap();
        s.close().unwrap();
        assert!(s.write(b"x").is_err());
    }
}

#[cfg(test)]
mod bounded_tests;

#[cfg(test)]
mod sync_publication_tests {
    use super::*;
    #[test]
    fn one_complete_frame_per_poll_budget_reserves_live_revision() {
        let mut t = Terminal::new(2, 20).unwrap();
        t.capture_sync_end = true;
        t.feed(b"\x1b[?2026hA\x1b[?2026l\x1b[?2026hB\x1b[?2026l\x1b[?2026hC")
            .unwrap();
        assert!(t.publication_held());
        let completed = t.completed_sync_frame.take().unwrap();
        assert_eq!(completed.rows()[0].cells[0].character, 'A');
        assert_eq!(completed.rows()[0].cells[1].character, ' ');
        assert!(t.revision > completed.stamp().revision);
        t.feed(b"\x1b[?2026l\x1b[?2026hD").unwrap();
        assert!(t.completed_sync_frame.is_none());
        t.sync_started = Some(std::time::Instant::now() - std::time::Duration::from_millis(251));
        assert!(!t.publication_held());
        assert!(t.frame().unwrap().stamp().revision > completed.stamp().revision);
    }
    #[test]
    fn fragmented_hold_repeated_begin_deadline_and_release() {
        let mut t = Terminal::new(2, 20).unwrap();
        t.feed(b"\x1b[?202").unwrap();
        assert!(!t.publication_held());
        t.feed(b"6hpartial").unwrap();
        assert!(t.publication_held());
        t.sync_started = Some(std::time::Instant::now() - std::time::Duration::from_millis(251));
        t.feed(b"\x1b[?2026h").unwrap();
        assert!(!t.publication_held());
        assert_eq!(t.feed(b"\x1b[?2026$p").unwrap(), b"\x1b[?2026;2$y");
        t.feed(b"\x1b[?2026l\x1b[?2026h").unwrap();
        assert!(t.publication_held());
        assert_eq!(t.feed(b"\x1b[?2026$p").unwrap(), b"\x1b[?2026;1$y");
        t.feed(b"\x1b[?2026l").unwrap();
        assert!(!t.publication_held());
        assert_eq!(t.feed(b"\x1b[?2026$p").unwrap(), b"\x1b[?2026;2$y");
    }
}
