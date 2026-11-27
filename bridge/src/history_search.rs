//! Aether transport for shared retained-history search.
use crate::{live::HistorySnapshot, protocol::MAX_HISTORY_SNAPSHOT_BYTES, semantic::Attachment};
use jterm_engine::{
    history_reflow::HistoryAnchor,
    history_search::{validate_query, HistorySearch as EngineSearch, HistorySearchProgress},
    terminal::Terminal,
};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySearchRequest {
    pub(crate) id: u32,
    pub(crate) query: String,
    pub(crate) restart: bool,
}

impl HistorySearchRequest {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.id == 0 {
            return Err("history search id must be positive".into());
        }
        validate_query(&self.query)
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistorySearchResponse {
    Match {
        id: u32,
        epoch: u32,
        snapshot: HistorySnapshot,
        #[serde(skip_serializing_if = "Option::is_none")]
        projection: Option<String>,
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

pub(crate) struct HistorySearch {
    pub(crate) id: u32,
    search: EngineSearch,
}

impl HistorySearch {
    pub(crate) fn new(id: u32, query: String) -> Result<Self, String> {
        Ok(Self {
            id,
            search: EngineSearch::new(query, MAX_HISTORY_SNAPSHOT_BYTES)?,
        })
    }

    pub(crate) fn query(&self) -> &str {
        self.search.query()
    }

    pub(crate) fn step(
        &mut self,
        terminal: &Terminal,
        epoch: u32,
        projection: Option<(Attachment, u8)>,
    ) -> Result<HistorySearchResponse, String> {
        let id = self.id;
        Ok(match self.search.step(terminal)? {
            HistorySearchProgress::Match(found) => {
                let projection = projection
                    .map(|(owner, version)| {
                        // The search cursor already checked that its original raw
                        // extent survived. Read the same owned rows at their
                        // original widths so a resize cannot shift match offsets.
                        let page = terminal.history_page(found.offset, found.row_count)?;
                        let first =
                            terminal.history_extent().end - (found.offset + found.row_count) as u64;
                        let cols = crate::semantic_v4::history_columns(&page, 1);
                        crate::semantic_v4::encode_history(
                            owner,
                            &page,
                            cols,
                            found.revision,
                            &found.text,
                            |index| {
                                Some(HistoryAnchor {
                                    row: first + index as u64,
                                    column: 0,
                                })
                            },
                            version,
                        )
                    })
                    .transpose()?;
                let start = found.text[..found.range.start].encode_utf16().count();
                let end = start + found.text[found.range].encode_utf16().count();
                HistorySearchResponse::Match {
                    id,
                    epoch,
                    snapshot: HistorySnapshot {
                        epoch,
                        revision: found.revision.to_string(),
                        offset: found.offset,
                        total_rows: found.total_rows,
                        row_count: Some(found.row_count),
                        text: found.text.to_string(),
                    },
                    projection,
                    start,
                    end,
                }
            }
            HistorySearchProgress::More => HistorySearchResponse::More { id, epoch },
            HistorySearchProgress::End => HistorySearchResponse::End { id, epoch },
        })
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
    fn projected_match_carries_the_raster_from_the_same_immutable_raw_window() {
        let mut terminal = Terminal::new(3, 20).unwrap();
        terminal.enable_inline_graphics((12, 24), |_, _, _| None);
        terminal
            .feed(b"\x1b_Ga=T,i=51,f=24,s=1,v=1,c=2,r=1,C=1;/wAA\x1b\\\x1b[1;6H")
            .unwrap();
        terminal.feed("🐍 needle\x1b[3S".as_bytes()).unwrap();
        terminal.resize(3, 12).unwrap();
        let owner = Attachment {
            session: "test".into(),
            epoch: 1,
            generation: 2,
        };
        let mut search = HistorySearch::new(1, "needle".into()).unwrap();
        let HistorySearchResponse::Match {
            snapshot,
            projection: Some(projection),
            start,
            end,
            ..
        } = search
            .step(
                &terminal,
                1,
                Some((owner.clone(), crate::protocol_v4::IMAGE_VERSION)),
            )
            .unwrap()
        else {
            panic!("history match missing");
        };
        let projection: crate::protocol_v4::HistoryView =
            serde_json::from_str(&projection).unwrap();
        assert_eq!(projection.owner, owner);
        assert_eq!(projection.cols, 20);
        assert_eq!(projection.text, snapshot.text);
        assert_eq!(projection.rows[0].source.row.get(), 1);
        assert_eq!(
            projection.rows[0].paint.image_slices[0].id,
            projection.images[0].id
        );
        assert_eq!(projection.images[0].rgba, "/wAA/w==");
        assert_eq!(
            String::from_utf16(&projection.text.encode_utf16().collect::<Vec<_>>()[start..end])
                .unwrap(),
            "needle"
        );
    }

    #[test]
    fn search_windows_join_soft_wraps_and_preserve_hard_newlines() {
        let mut soft = history_with_boundary(b"ABCDWXYZ\r\n");
        let hard = history_with_boundary(b"ABCD\r\nWXYZ\r\n");
        let mut wraps = HistorySearch::new(1, "CDWX".into()).unwrap();
        assert!(matches!(
            wraps.step(&soft, 2, None).unwrap(),
            HistorySearchResponse::More { .. }
        ));
        soft.feed(b"newer\r\n").unwrap();
        let HistorySearchResponse::Match {
            snapshot,
            start,
            end,
            ..
        } = wraps.step(&soft, 2, None).unwrap()
        else {
            panic!("a soft-wrap match was lost at the search boundary");
        };
        assert_eq!(snapshot.offset, 66);
        assert_eq!(&snapshot.text[start..end], "CDWX");
        let mut preserves_newline = HistorySearch::new(1, "CDWX".into()).unwrap();
        assert!(matches!(
            preserves_newline.step(&hard, 2, None).unwrap(),
            HistorySearchResponse::More { .. }
        ));
        assert!(matches!(
            preserves_newline.step(&hard, 2, None).unwrap(),
            HistorySearchResponse::End { .. }
        ));
    }

    #[test]
    fn search_ranges_use_utf16_and_keep_the_same_archive_across_resize() {
        let mut terminal = Terminal::new(2, 16).unwrap();
        terminal.feed("🐍 中尾 中尾\r\n\r\n".as_bytes()).unwrap();
        let mut search = HistorySearch::new(3, "中尾".into()).unwrap();
        let HistorySearchResponse::Match {
            snapshot,
            start,
            end,
            ..
        } = search.step(&terminal, 9, None).unwrap()
        else {
            panic!("missing match");
        };
        assert_eq!(start, 6);
        let units: Vec<_> = snapshot.text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&units[start..end]).unwrap(), "中尾");
        terminal.resize(3, 10).unwrap();
        let HistorySearchResponse::Match {
            snapshot: older,
            start,
            ..
        } = search.step(&terminal, 9, None).unwrap()
        else {
            panic!("resize lost the older match in the immutable source");
        };
        assert_eq!(start, 3);
        assert_eq!(older.revision, snapshot.revision);
        terminal.feed(b"\x1b[3J").unwrap();
        assert_eq!(
            search.step(&terminal, 9, None).unwrap_err(),
            "History changed. Start a new search."
        );
    }

    #[test]
    fn overlapping_occurrences_remain_ordered_by_start() {
        let mut terminal = Terminal::new(2, 8).unwrap();
        terminal.feed(b"aaaa\r\n\r\n").unwrap();
        let mut search = HistorySearch::new(1, "aa".into()).unwrap();
        for expected in [2, 1, 0] {
            let HistorySearchResponse::Match { start, .. } =
                search.step(&terminal, 1, None).unwrap()
            else {
                panic!("older match missing");
            };
            assert_eq!(start, expected);
        }
        assert!(matches!(
            search.step(&terminal, 1, None).unwrap(),
            HistorySearchResponse::End { .. }
        ));
    }

    #[test]
    fn search_pages_dense_unicode_without_emitting_an_oversized_snapshot() {
        let mut terminal = Terminal::new(2, 256).unwrap();
        terminal.enable_grapheme_clusters();
        let body = "e\u{301}\u{302}\u{303}\u{304}\u{305}".repeat(240);
        terminal
            .feed(format!("{body} needle\r\n").as_bytes())
            .unwrap();
        for _ in 1..85 {
            terminal.feed(format!("{body}\r\n").as_bytes()).unwrap();
        }
        let mut search = HistorySearch::new(7, "needle".into()).unwrap();
        assert!(matches!(
            search.step(&terminal, 4, None).unwrap(),
            HistorySearchResponse::More { .. }
        ));
        assert!(matches!(
            search.step(&terminal, 4, None).unwrap(),
            HistorySearchResponse::More { .. }
        ));
        let HistorySearchResponse::Match {
            snapshot,
            start,
            end,
            ..
        } = search.step(&terminal, 4, None).unwrap()
        else {
            panic!("oldest dense window was lost");
        };
        assert_eq!(snapshot.offset, 64);
        assert!(snapshot.text.len() <= MAX_HISTORY_SNAPSHOT_BYTES);
        let units: Vec<_> = snapshot.text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&units[start..end]).unwrap(), "needle");
    }
}
