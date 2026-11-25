use super::LiveBridge;
use crate::{protocol_v4::Counter, semantic::Attachment};
use jterm_engine::command_blocks::{CommandBlock, CommandOutputSource, OutputUnavailable};
use sha2::{Digest, Sha256};

#[derive(serde::Serialize)]
pub struct CommandBlockSnapshot {
    owner: Attachment,
    enabled: bool,
    blocks: Vec<RetainedBlock>,
    bookmarked: Vec<String>,
}

pub struct CommandActivity {
    pub alternate_screen: bool,
    pub prompt: Option<Counter>,
    pub running: bool,
    pub newest_completed: Option<Counter>,
    pub newest_exit: Option<i32>,
}

#[derive(serde::Serialize)]
pub struct RetainedBlock {
    pub sequence: String,
    pub execution_id: String,
    pub shell_session: Option<String>,
    pub shell_sequence: Option<String>,
    pub command: String,
    pub cwd: Option<String>,
    pub exit: Option<i32>,
    pub provenance: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_source: Option<RetainedOutputSource>,
    output: Output,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revision: String,
}

#[derive(serde::Serialize)]
pub struct RetainedOutputSource {
    pub byte_start: String,
    pub byte_end: String,
    pub start_event: String,
    pub end_event: String,
    pub start_size: (usize, usize),
    pub end_size: (usize, usize),
    pub resized: bool,
}

impl From<&CommandOutputSource> for RetainedOutputSource {
    fn from(source: &CommandOutputSource) -> Self {
        Self {
            byte_start: source.bytes.start.to_string(),
            byte_end: source.bytes.end.to_string(),
            start_event: source.start_event.to_string(),
            end_event: source.end_event.to_string(),
            start_size: source.start_size,
            end_size: source.end_size,
            resized: source.resized,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockSelection {
    pub sequence: String,
    pub revision: String,
}

impl BlockSelection {
    fn matches(&self, block: &RetainedBlock) -> bool {
        self.sequence == block.sequence && self.revision == block.revision
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Output {
    Text { text: String },
    ScreenOperation,
    ByteLimit,
}

impl From<&CommandBlock> for RetainedBlock {
    fn from(record: &CommandBlock) -> Self {
        let output = match &record.output {
            Ok(text) => Output::Text { text: text.clone() },
            Err(OutputUnavailable::ScreenOperation) => Output::ScreenOperation,
            Err(OutputUnavailable::ByteLimit) => Output::ByteLimit,
        };
        let mut block = Self {
            sequence: record.sequence.to_string(),
            execution_id: record.execution_id.clone(),
            shell_session: record.shell_session.clone(),
            shell_sequence: record.shell_sequence.map(|sequence| sequence.to_string()),
            command: record.command.clone(),
            cwd: record.cwd.clone(),
            exit: record.outcome.reported_exit_code(),
            provenance: record.provenance.schema_name(),
            output_source: record
                .output_source
                .as_ref()
                .map(RetainedOutputSource::from),
            output,
            revision: String::new(),
        };
        // Include every retained field, including capture availability.
        // The empty revision is omitted from this stable wire projection.
        let wire = serde_json::to_vec(&block).expect("command block projection is JSON");
        block.revision = format!("{:x}", Sha256::digest(wire));
        block
    }
}

impl RetainedBlock {
    pub fn captured_output(&self) -> Option<&str> {
        match &self.output {
            Output::Text { text } => Some(text),
            Output::ScreenOperation | Output::ByteLimit => None,
        }
    }
}

impl CommandBlockSnapshot {
    pub fn retained_blocks(&self) -> &[RetainedBlock] {
        &self.blocks
    }

    /// Take two exact complete records from one already-validated attachment.
    pub fn select_pair(
        self,
        before: &BlockSelection,
        after: &BlockSelection,
    ) -> Result<(RetainedBlock, RetainedBlock), String> {
        if before.sequence == after.sequence {
            return Err("Choose two different completed commands.".into());
        }
        let mut blocks = self.blocks;
        let mut select = |choice: &BlockSelection| {
            let index = blocks
                .iter()
                .position(|record| choice.matches(record))
                .ok_or(
                    "A selected command revision is no longer retained. Refresh the command list.",
                )?;
            let record = blocks.swap_remove(index);
            record
                .captured_output()
                .ok_or("A selected command has no retained plain output.")?;
            Ok::<_, String>(record)
        };
        Ok((select(before)?, select(after)?))
    }
}

impl LiveBridge {
    pub fn command_activity(&self, epoch: u32) -> Result<CommandActivity, String> {
        self.check(epoch)?;
        let terminal = self
            .session
            .as_ref()
            .expect("validated local PTY")
            .terminal();
        let latest = terminal
            .completed_command_blocks()
            .and_then(|blocks| blocks.back());
        Ok(CommandActivity {
            alternate_screen: terminal.alternate_screen_active(),
            prompt: terminal.command_prompt_generation().and_then(Counter::new),
            running: terminal.command_running(),
            newest_completed: latest.and_then(|block| Counter::new(block.sequence)),
            newest_exit: latest.and_then(|block| block.outcome.reported_exit_code()),
        })
    }

    /// The caller holds the attachment's writable input lease. Advance PTY
    /// state before checking the displayed prompt and its foreground group.
    pub fn submit_prompt_enter(&mut self, epoch: u32, prompt: Counter) -> Result<bool, String> {
        if !self.prompt_accepts_input(epoch, prompt)? {
            return Ok(false);
        }
        self.key(epoch, "Enter", false, false, false)?;
        // Keep an accepted action spent even if its receipt is lost or the
        // shell's keyboard encoding does not contain a literal carriage return.
        self.submitted_chat_prompt = Some(prompt);
        Ok(true)
    }

    /// Insert a rendered workflow at the live cursor using one ordered paste event.
    pub fn insert_prompt_workflow(
        &mut self,
        epoch: u32,
        prompt: Counter,
        command: &str,
    ) -> Result<(), String> {
        if command.trim().is_empty() || command.chars().any(char::is_control) {
            return Err("Workflow insertion requires a single line of text.".into());
        }
        if !self.prompt_accepts_input(epoch, prompt)? {
            return Err("The original shell prompt can no longer receive this workflow.".into());
        }
        let session = self.session.as_mut().expect("validated local PTY");
        if !session.terminal().bracketed_paste_active() {
            return Err(
                "The shell prompt must support bracketed paste for workflow insertion.".into(),
            );
        }
        session.submit_native_paste(command)?;
        Ok(())
    }

    fn prompt_accepts_input(&mut self, epoch: u32, prompt: Counter) -> Result<bool, String> {
        self.check(epoch)?;
        if !self.input_attached || self.submitted_chat_prompt == Some(prompt) {
            return Ok(false);
        }
        if self.flight.is_some() {
            self.pump_attached_io(epoch)?;
        } else {
            self.pump_unpublished()?;
        }
        let session = self.session.as_ref().expect("validated local PTY");
        Ok(!session.is_exited()
            && session.terminal().command_prompt_generation() == Some(prompt.get())
            && session.at_shell_prompt())
    }

    /// Resolve the displayed revision at delivery, while holding the same session
    /// owner as ordinary paste. A stale card or a foreground program receives no input.
    pub fn recall_command(
        &mut self,
        epoch: u32,
        choice: &BlockSelection,
    ) -> Result<Option<&'static str>, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().expect("validated local PTY");
        if !session.at_shell_prompt() {
            return Ok(Some(
                "Wait for the integrated shell prompt before inserting a command.",
            ));
        }
        let Some(command) = self
            .command_blocks(epoch)?
            .blocks
            .into_iter()
            .find(|block| choice.matches(block))
            .map(|block| block.command)
        else {
            return Ok(Some(
                "The selected command revision has expired. Refresh the command list.",
            ));
        };
        self.paste(epoch, &command)?;
        Ok(None)
    }

    /// Modify presentation metadata for an exact retained command in this PTY.
    pub fn set_command_bookmark(
        &mut self,
        epoch: u32,
        choice: BlockSelection,
        bookmarked: bool,
    ) -> Result<(), String> {
        let snapshot = self.command_blocks(epoch)?;
        if !snapshot.blocks.iter().any(|block| choice.matches(block)) {
            return Err(
                "The selected command revision has expired. Refresh the command list.".into(),
            );
        }
        self.command_bookmarks.retain(|sequence, revision| {
            snapshot
                .blocks
                .iter()
                .any(|block| &block.sequence == sequence && &block.revision == revision)
        });
        if bookmarked {
            self.command_bookmarks
                .insert(choice.sequence, choice.revision);
        } else {
            self.command_bookmarks.remove(&choice.sequence);
        }
        Ok(())
    }

    /// Independent read-only projection of the current session's bounded records.
    pub fn command_blocks(&self, epoch: u32) -> Result<CommandBlockSnapshot, String> {
        let owner = self.attachment(epoch)?;
        let records = self
            .session
            .as_ref()
            .expect("validated local PTY")
            .terminal()
            .completed_command_blocks();
        let blocks: Vec<_> = records
            .into_iter()
            .flat_map(|blocks| blocks.iter().rev().map(RetainedBlock::from))
            .collect();
        let bookmarked = blocks
            .iter()
            .filter(|block| self.command_bookmarks.get(&block.sequence) == Some(&block.revision))
            .map(|block| block.sequence.clone())
            .collect();
        Ok(CommandBlockSnapshot {
            owner,
            enabled: records.is_some(),
            blocks,
            bookmarked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_engine::block_contract::{CompletedBlockOutcome, CompletionProvenance};

    fn record(sequence: u64, output: Result<&str, OutputUnavailable>) -> RetainedBlock {
        RetainedBlock::from(&CommandBlock {
            sequence,
            execution_id: format!("run-{sequence}"),
            shell_session: None,
            shell_sequence: None,
            command: "printf fixture".into(),
            cwd: None,
            outcome: CompletedBlockOutcome::Success,
            provenance: CompletionProvenance::ShellReported,
            output: output.map(ToOwned::to_owned),
            output_source: None,
        })
    }

    #[test]
    fn captured_sources_preserve_64_bit_identity_in_the_wire_projection() {
        let mut terminal = jterm_engine::terminal::Terminal::new(24, 80).unwrap();
        terminal.observe_command_blocks();
        let start = b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;C;id=run;command=printf%20fixture\x07";
        let output = b"captured\r\n";
        let end = b"\x1b]133;D;0;id=run\x1b\\";
        for bytes in [start.as_slice(), output, end] {
            terminal.feed(bytes).unwrap();
        }
        let mut record = terminal.completed_command_blocks().unwrap()[0].clone();
        let wire =
            |record: &CommandBlock| serde_json::to_value(RetainedBlock::from(record)).unwrap();
        let first = wire(&record);
        assert_eq!(first["execution_id"], "run");
        assert_eq!(
            first["output_source"]["byte_start"],
            start.len().to_string()
        );
        assert_eq!(
            first["output_source"]["byte_end"],
            (start.len() + output.len()).to_string()
        );
        assert_eq!(first["output_source"]["start_event"], "3");
        assert_eq!(
            first["output_source"]["start_size"],
            serde_json::json!([80, 24])
        );

        let source = record.output_source.as_mut().unwrap();
        source.bytes = u64::MAX - 9..u64::MAX - 1;
        let second = wire(&record);
        assert_eq!(
            second["output_source"]["byte_start"],
            (u64::MAX - 9).to_string()
        );
        assert_ne!(first["revision"], second["revision"]);
    }

    #[test]
    fn selects_distinct_exact_revisions_and_rejects_unavailable_output() {
        let snapshot = || CommandBlockSnapshot {
            owner: Attachment {
                session: "fixture".into(),
                epoch: 1,
                generation: 1,
            },
            enabled: true,
            blocks: vec![
                record(3, Err(OutputUnavailable::ByteLimit)),
                record(2, Ok("")),
                record(1, Ok("before\n")),
            ],
            bookmarked: Vec::new(),
        };
        let selection = |record: RetainedBlock| BlockSelection {
            sequence: record.sequence,
            revision: record.revision,
        };
        let before = selection(record(1, Ok("before\n")));
        let after = selection(record(2, Ok("")));
        let (first, second) = snapshot().select_pair(&before, &after).unwrap();
        assert_eq!(first.captured_output(), Some("before\n"));
        assert_eq!(second.captured_output(), Some(""));
        assert!(snapshot().select_pair(&before, &before).is_err());
        assert!(snapshot()
            .select_pair(&selection(record(1, Ok("changed"))), &after)
            .is_err());
        assert!(snapshot()
            .select_pair(
                &before,
                &selection(record(3, Err(OutputUnavailable::ByteLimit)))
            )
            .is_err());
    }
}
