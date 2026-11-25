use jterm_core::cross_block_search::{utf8_prefix, FindScanBudget};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::WebviewWindow;
use tauri_terminal_frame_bridge::{
    live::{BlockSelection, CommandBlockSnapshot},
    semantic::Attachment,
};

use crate::local_sessions::LocalSessions;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BlockMatches {
    hits: Vec<BlockHit>,
    scan_incomplete: bool,
}

#[derive(Serialize)]
struct BlockHit {
    record: BlockSelection,
    label: String,
    surface: Surface,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
enum Surface {
    Command,
    Output,
}

#[tauri::command]
pub(crate) async fn search_local_blocks<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    owner: Attachment,
    query: String,
    bridge: tauri::State<'_, Arc<Mutex<LocalSessions>>>,
) -> Result<BlockMatches, String> {
    crate::require_terminal_window(&window)?;
    jterm_engine::history_search::validate_query(&query)
        .map_err(|_| "Enter a literal query of 1–128 UTF-8 bytes without control characters.")?;
    // Capture a bounded set of immutable records while the attachment is
    // current. The scan runs after releasing the live PTY registry lock.
    let snapshot = crate::run_attached(bridge.inner().clone(), owner, |bridge, epoch| {
        bridge.command_blocks(epoch)
    })
    .await?;
    tauri::async_runtime::spawn_blocking(move || search(&snapshot, &query))
        .await
        .map_err(|_| "The command search worker is unavailable.".into())
}

fn search(snapshot: &CommandBlockSnapshot, query: &str) -> BlockMatches {
    let mut hits = Vec::new();
    let mut budget = FindScanBudget::for_cross_block();
    let mut scan_incomplete = false;
    for block in snapshot.retained_blocks() {
        let label = block.command.lines().next().unwrap_or_default();
        let mut label = utf8_prefix(label, 160).to_owned();
        if label.len() < block.command.len() {
            label.push('…');
        }
        let output = block.captured_output();
        scan_incomplete |= output.is_none();
        let surfaces = [
            (Surface::Command, Some(block.command.as_str())),
            (Surface::Output, output),
        ];
        for (surface, text) in surfaces {
            let Some(text) = text else { continue };
            let prefix = budget.take_prefix(text);
            scan_incomplete |= prefix.incomplete;
            if let Some(offset) = prefix.text.find(query) {
                let start = text[..offset].encode_utf16().count();
                hits.push(BlockHit {
                    record: BlockSelection {
                        sequence: block.sequence.clone(),
                        revision: block.revision.clone(),
                    },
                    label: label.clone(),
                    surface,
                    start,
                    end: start + query.encode_utf16().count(),
                });
            }
        }
    }
    BlockMatches {
        hits,
        scan_incomplete,
    }
}
