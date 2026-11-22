//! Incremental literal search over the immutable shared-engine history view.
use crate::live::HistorySnapshot;
use jterm_engine::terminal::Terminal;

const SEARCH_ROWS: usize = 64;
const MAX_QUERY_BYTES: usize = 128;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySearchRequest {
    pub(crate) id: u32,
    pub(crate) query: String,
    pub(crate) restart: bool,
}

impl HistorySearchRequest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.id == 0
            || self.query.is_empty()
            || self.query.len() > MAX_QUERY_BYTES
            || self.query.chars().any(char::is_control)
        {
            return Err(
                "Enter a literal history query of 1–128 UTF-8 bytes without control characters."
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistorySearchResponse {
    Match {
        id: u32,
        epoch: u32,
        snapshot: HistorySnapshot,
        start: usize,
        end: usize,
    },
    More {
        id: u32,
        epoch: u32,
    },
    End {
        id: u32,
        epoch: u32,
    },
}

struct Window {
    text: String,
    owned_bytes: usize,
    before: usize,
    offset: usize,
    total_rows: usize,
}

fn char_boundary_before(text: &str, mut limit: usize) -> usize {
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    limit
}

pub(crate) struct HistorySearch {
    pub(crate) id: u32,
    pub(crate) query: String,
    revision: Option<u64>,
    offset: usize,
    // The beginning of the already searched newer text. It only contributes
    // to a match that starts in the current window.
    newer_prefix: String,
    window: Option<Window>,
}

impl HistorySearch {
    pub(crate) fn new(id: u32, query: String) -> Self {
        Self {
            id,
            query,
            revision: None,
            offset: 0,
            newer_prefix: String::new(),
            window: None,
        }
    }

    pub(crate) fn step(
        &mut self,
        terminal: &Terminal,
        epoch: u32,
    ) -> Result<HistorySearchResponse, String> {
        if self.window.is_some() {
            self.check_revision(terminal.history_page(0, 1)?.revision)?;
        } else {
            let sentinel = usize::from(self.offset > 0);
            let page = terminal.history_page(self.offset - sentinel, SEARCH_ROWS + sentinel)?;
            self.check_revision(page.revision)?;
            let row_count = page.row_count.saturating_sub(sentinel);
            if row_count == 0 {
                return Ok(HistorySearchResponse::End { id: self.id, epoch });
            }

            // A following row makes selection_text include the real boundary
            // after our final row, preserving both hard newlines and soft wraps.
            let mut text = if sentinel == 0 {
                page.text
            } else {
                page.selection_text(0, 0, row_count, 0)?
            };
            let owned_bytes = text.len();
            text.push_str(&self.newer_prefix);
            let prefix = char_boundary_before(&text, text.len().min(self.query.len() - 1));
            self.newer_prefix.clear();
            self.newer_prefix.push_str(&text[..prefix]);
            self.window = Some(Window {
                before: text.len(),
                text,
                owned_bytes,
                offset: self.offset,
                total_rows: page.total_rows,
            });
            self.offset += row_count;
        }

        let window = self.window.as_mut().expect("loaded history window");
        // Limit by match start so even overlapping older occurrences can be
        // selected without repeating the current match.
        let limit = char_boundary_before(
            &window.text,
            (window.before + self.query.len() - 1).min(window.text.len()),
        );
        if let Some((start, _)) = window.text[..limit]
            .rmatch_indices(&self.query)
            .find(|(start, _)| *start < window.before.min(window.owned_bytes))
        {
            window.before = start;
            let utf16_start = window.text[..start].encode_utf16().count();
            return Ok(HistorySearchResponse::Match {
                id: self.id,
                epoch,
                snapshot: HistorySnapshot {
                    epoch,
                    revision: self.revision.expect("loaded history revision").to_string(),
                    offset: window.offset,
                    total_rows: window.total_rows,
                    text: window.text.clone(),
                },
                start: utf16_start,
                end: utf16_start + self.query.encode_utf16().count(),
            });
        }
        let remaining = self.offset < window.total_rows;
        self.window = None;
        Ok(if remaining {
            HistorySearchResponse::More { id: self.id, epoch }
        } else {
            HistorySearchResponse::End { id: self.id, epoch }
        })
    }

    fn check_revision(&mut self, revision: u64) -> Result<(), String> {
        if self.revision.is_some_and(|expected| expected != revision) {
            return Err("History changed. Start a new search.".into());
        }
        self.revision = Some(revision);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_with_boundary(first_rows: &[u8]) -> Terminal {
        let mut terminal = Terminal::new(2, 4).unwrap();
        terminal.feed(first_rows).unwrap();
        assert_eq!(terminal.history_rows(), 1);
        terminal.feed(&b"k\r\n".repeat(64)).unwrap();
        assert_eq!(terminal.history_rows(), 65);
        terminal
    }

    #[test]
    fn search_windows_join_soft_wraps_and_preserve_hard_newlines() {
        let soft = history_with_boundary(b"ABCDWXYZ\r\n");
        let hard = history_with_boundary(b"ABCD\r\nWXYZ\r\n");
        let mut wraps = HistorySearch::new(1, "CDWX".into());
        assert!(matches!(
            wraps.step(&soft, 2).unwrap(),
            HistorySearchResponse::More { .. }
        ));
        let HistorySearchResponse::Match {
            snapshot,
            start,
            end,
            ..
        } = wraps.step(&soft, 2).unwrap()
        else {
            panic!("a soft-wrap match was lost at the search boundary");
        };
        assert_eq!(snapshot.offset, 64);
        assert_eq!(&snapshot.text[start..end], "CDWX");
        let mut preserves_newline = HistorySearch::new(1, "CDWX".into());
        assert!(matches!(
            preserves_newline.step(&hard, 2).unwrap(),
            HistorySearchResponse::More { .. }
        ));
        assert!(matches!(
            preserves_newline.step(&hard, 2).unwrap(),
            HistorySearchResponse::End { .. }
        ));
    }

    #[test]
    fn search_ranges_use_utf16_and_resize_invalidates_older_navigation() {
        let mut terminal = Terminal::new(2, 16).unwrap();
        terminal.feed("🐍 中尾 中尾\r\n\r\n".as_bytes()).unwrap();
        let mut search = HistorySearch::new(3, "中尾".into());
        let HistorySearchResponse::Match {
            snapshot,
            start,
            end,
            ..
        } = search.step(&terminal, 9).unwrap()
        else {
            panic!("missing match");
        };
        assert_eq!(start, 6);
        let units: Vec<_> = snapshot.text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&units[start..end]).unwrap(), "中尾");
        terminal.resize(3, 16).unwrap();
        assert_eq!(
            search.step(&terminal, 9).unwrap_err(),
            "History changed. Start a new search."
        );
    }

    #[test]
    fn overlapping_occurrences_remain_ordered_by_start() {
        let mut terminal = Terminal::new(2, 8).unwrap();
        terminal.feed(b"aaaa\r\n\r\n").unwrap();
        let mut search = HistorySearch::new(1, "aa".into());
        for expected in [2, 1, 0] {
            let HistorySearchResponse::Match { start, .. } = search.step(&terminal, 1).unwrap()
            else {
                panic!("older match missing");
            };
            assert_eq!(start, expected);
        }
        assert!(matches!(
            search.step(&terminal, 1).unwrap(),
            HistorySearchResponse::End { .. }
        ));
    }
}
