//! Frozen comparison of two exact retained local PTY records.

use jterm_core::output_compare::{compare, OutputSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt::Write;
use std::sync::{Arc, Mutex};
use tauri::WebviewWindow;
use tauri_terminal_frame_bridge::live::{BlockSelection, RetainedBlock};
use tauri_terminal_frame_bridge::semantic::Attachment;
use uuid::Uuid;

const MAX_FROZEN_REVIEWS: usize = 4;
const EXPORT_SCHEMA: &str = "aether.command-output-review.v1";

#[derive(Default)]
pub struct OutputReviews(VecDeque<FrozenReview>);

struct FrozenReview {
    receipt: ReviewReceipt,
    before: RetainedBlock,
    after: RetainedBlock,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReceipt {
    review_id: Uuid,
    owner: Attachment,
    summary: String,
    comparison: String,
    complete: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Markdown,
    Json,
}

#[derive(Serialize)]
pub struct ExportedReview {
    filename: String,
    content: String,
}

#[tauri::command]
pub async fn compare_local_output<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    owner: Attachment,
    before: BlockSelection,
    after: BlockSelection,
    sessions: tauri::State<'_, Arc<Mutex<super::LocalSessions>>>,
    reviews: tauri::State<'_, Arc<Mutex<OutputReviews>>>,
) -> Result<ReviewReceipt, String> {
    super::require_terminal_window(&window)?;
    let (before, after) = super::run_attached(
        sessions.inner().clone(),
        owner.clone(),
        move |bridge, epoch| bridge.command_blocks(epoch)?.select_pair(&before, &after),
    )
    .await?;
    super::run_bridge(reviews.inner().clone(), move |reviews| {
        reviews.freeze(owner, before, after)
    })
    .await
}

#[tauri::command]
pub async fn export_local_output<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    review_id: Uuid,
    format: ExportFormat,
    reviews: tauri::State<'_, Arc<Mutex<OutputReviews>>>,
) -> Result<ExportedReview, String> {
    super::require_terminal_window(&window)?;
    super::run_bridge(reviews.inner().clone(), move |reviews| {
        reviews.export(review_id, format)
    })
    .await
}

impl OutputReviews {
    fn freeze(
        &mut self,
        owner: Attachment,
        before: RetainedBlock,
        after: RetainedBlock,
    ) -> Result<ReviewReceipt, String> {
        let diff = compare(
            OutputSnapshot {
                text: before
                    .captured_output()
                    .ok_or("No retained before output")?,
                truncated: false,
            },
            OutputSnapshot {
                text: after.captured_output().ok_or("No retained after output")?,
                truncated: false,
            },
        );
        let receipt = ReviewReceipt {
            review_id: Uuid::new_v4(),
            owner,
            summary: diff.summary(),
            complete: diff.is_complete(),
            comparison: diff.text,
        };
        let response = receipt.clone();
        if self.0.len() == MAX_FROZEN_REVIEWS {
            self.0.pop_front();
        }
        self.0.push_back(FrozenReview {
            receipt,
            before,
            after,
        });
        Ok(response)
    }

    fn export(&self, id: Uuid, format: ExportFormat) -> Result<ExportedReview, String> {
        let review = self
            .0
            .iter()
            .find(|frozen| frozen.receipt.review_id == id)
            .ok_or("This frozen output review is no longer available.")?;
        let (extension, content) = match format {
            ExportFormat::Markdown => ("md", review.markdown()),
            ExportFormat::Json => ("json", review.json()?),
        };
        Ok(ExportedReview {
            filename: format!("aether-output-{id}.{extension}"),
            content,
        })
    }
}

impl FrozenReview {
    fn markdown(&self) -> String {
        let mut out = String::new();
        let receipt = &self.receipt;
        let owner = &receipt.owner;
        let _ = write!(out, "# Frozen command output review\n\nReview {}\n\nLocal PTY `{}` · epoch {} · attachment {}\n\n{}\n\n",
            receipt.review_id, owner.session, owner.epoch, owner.generation, receipt.summary);
        for (heading, record) in [("Before", &self.before), ("After", &self.after)] {
            let _ = write!(
                out,
                "## {heading} · block {}\n\nRevision `{}`\n\n",
                record.sequence, record.revision
            );
            let exit = record
                .exit
                .map_or_else(|| "unavailable".to_owned(), |code| code.to_string());
            let _ = writeln!(out, "PTY-reported OSC 133 · exit {exit}\n");
            if let Some(source) = &record.output_source {
                if let Some(identity) = &source.identity {
                    let _ = writeln!(
                        out,
                        "PTY stream `{}` · execution `{}`\n",
                        identity.stream_id, identity.execution_id
                    );
                }
                let (start_cols, start_rows) = source.start_size;
                let (end_cols, end_rows) = source.end_size;
                let _ = writeln!(
                    out,
                    "PTY bytes `[{}, {})` · marker events {}–{} · grid {start_cols}×{start_rows} to {end_cols}×{end_rows}\n",
                    source.byte_start, source.byte_end, source.start_event, source.end_event,
                );
            }
            append_literal(&mut out, "Command", &record.command);
            if let Some(cwd) = &record.cwd {
                append_literal(&mut out, "Working directory", cwd);
            }
            append_literal(
                &mut out,
                "Retained output",
                record.captured_output().expect("frozen output"),
            );
        }
        append_literal(&mut out, "Comparison", &receipt.comparison);
        out
    }

    fn json(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct Document<'a> {
            schema: &'static str,
            review: &'a ReviewReceipt,
            before: &'a RetainedBlock,
            after: &'a RetainedBlock,
        }
        serde_json::to_string_pretty(&Document {
            schema: EXPORT_SCHEMA,
            review: &self.receipt,
            before: &self.before,
            after: &self.after,
        })
        .map_err(|error| error.to_string())
    }
}

fn append_literal(out: &mut String, heading: &str, contents: &str) {
    // A retained command or output can itself contain Markdown fences.
    let longest = contents
        .as_bytes()
        .split(|byte| *byte != b'`')
        .map(<[u8]>::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let _ = write!(out, "### {heading}\n\n{fence}\n{contents}");
    if !contents.ends_with('\n') {
        out.push('\n');
    }
    let _ = write!(out, "{fence}\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_engine::block_contract::{CompletedBlockOutcome, CompletionProvenance};
    use jterm_engine::command_blocks::CommandBlock;

    fn record(sequence: u64, output: &str) -> RetainedBlock {
        RetainedBlock::from(&CommandBlock {
            sequence,
            execution_id: format!("run-{sequence}"),
            shell_session: None,
            shell_sequence: None,
            command: "printf fixture".into(),
            cwd: Some("/a local directory".into()),
            outcome: CompletedBlockOutcome::Success,
            provenance: CompletionProvenance::ShellReported,
            output: Ok(output.into()),
            output_annotations: None,
            output_source: Some(jterm_engine::command_blocks::CommandOutputSource {
                bytes: 101..201,
                start_event: 3,
                end_event: 4,
                start_size: (80, 24),
                end_size: (80, 24),
                resized: false,
            }),
        })
    }

    #[test]
    fn exports_only_the_frozen_pair_and_escapes_markdown_fences() {
        let owner = Attachment {
            session: "aether-fixture".into(),
            epoch: 2,
            generation: 8,
        };
        let mut reviews = OutputReviews::default();
        let before = record(11, "less\n");
        let after = record(12, "more\n\x60\x60\x60\x60\n</script>\n");
        let revision = after.revision.clone();
        let old = reviews.freeze(owner.clone(), before, after).unwrap();
        reviews
            .freeze(owner.clone(), record(13, "newer"), record(14, "current"))
            .unwrap();
        let json = reviews.export(old.review_id, ExportFormat::Json).unwrap();
        let document: serde_json::Value = serde_json::from_str(&json.content).unwrap();
        assert_eq!(document["schema"], EXPORT_SCHEMA);
        assert_eq!(
            document["after"]["output"]["text"],
            "more\n\x60\x60\x60\x60\n</script>\n"
        );
        assert_eq!(document["after"]["revision"], revision);
        assert_eq!(document["after"]["output_source"]["byte_start"], "101");
        assert_eq!(document["after"]["output_source"]["end_event"], "4");
        assert_eq!(
            document["after"]["output_source"]["start_size"],
            serde_json::json!([80, 24])
        );
        assert_eq!(document["review"]["owner"]["generation"], 8);
        assert!(!json.content.contains("current"));
        let markdown = reviews
            .export(old.review_id, ExportFormat::Markdown)
            .unwrap();
        assert!(markdown.content.contains(
            "\x60\x60\x60\x60\x60\nmore\n\x60\x60\x60\x60\n</script>\n\x60\x60\x60\x60\x60"
        ));
        assert!(markdown.content.contains(&revision));
        assert!(markdown.content.contains("-less\n+more\n"));
        for _ in 0..MAX_FROZEN_REVIEWS {
            reviews
                .freeze(owner.clone(), record(15, "next"), record(16, "new"))
                .unwrap();
        }
        assert!(reviews.export(old.review_id, ExportFormat::Json).is_err());
    }
}
