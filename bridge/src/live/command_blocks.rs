use super::LiveBridge;
use crate::{protocol_v4::Counter, semantic::Attachment};
use jterm_engine::{
    command_blocks::{CommandBlock, CommandOutputSource, OutputUnavailable},
    shell_markers,
};
use jterm_protocol::finished_output::OutputAnnotations;
use sha2::{Digest, Sha256};

const MAX_BLOCKS: usize = 8;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(serde::Serialize)]
pub struct CommandBlockSnapshot {
    owner: Attachment,
    enabled: bool,
    blocks: Vec<RetainedBlock>,
    bookmarked: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    restored: Vec<String>,
}

pub struct CommandActivity {
    pub alternate_screen: bool,
    pub prompt: Option<Counter>,
    pub running: bool,
    pub newest_completed: Option<Counter>,
    pub newest_exit: Option<i32>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedBlock {
    pub sequence: String,
    pub execution_id: String,
    pub shell_session: Option<String>,
    pub shell_sequence: Option<String>,
    pub command: String,
    pub cwd: Option<String>,
    pub exit: Option<i32>,
    pub provenance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_source: Option<RetainedOutputSource>,
    output: Output,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_annotations: Option<OutputAnnotations>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revision: String,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct RetainedOutputSource {
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub identity: Option<OutputSourceIdentity>,
    pub byte_start: String,
    pub byte_end: String,
    pub start_event: String,
    pub end_event: String,
    pub start_size: (usize, usize),
    pub end_size: (usize, usize),
    pub resized: bool,
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct OutputSourceIdentity {
    pub stream_id: String,
    pub execution_id: String,
}

impl OutputSourceIdentity {
    fn new(stream: &str, sequence: u64) -> Self {
        let mut identity = Sha256::new();
        identity.update(b"aether.command-output-source.v1\0");
        identity.update(stream.as_bytes());
        identity.update(sequence.to_be_bytes());
        Self {
            stream_id: stream.to_owned(),
            execution_id: format!("{:x}", identity.finalize()),
        }
    }
}

impl From<&CommandOutputSource> for RetainedOutputSource {
    fn from(source: &CommandOutputSource) -> Self {
        Self {
            identity: None,
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

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Output {
    Text { text: String },
    ScreenOperation,
    ByteLimit,
}

impl From<&CommandBlock> for RetainedBlock {
    fn from(record: &CommandBlock) -> Self {
        Self::project(record, record.sequence, None)
    }
}

impl RetainedBlock {
    fn project(record: &CommandBlock, sequence: u64, stream: Option<&str>) -> Self {
        let output = match &record.output {
            Ok(text) => Output::Text { text: text.clone() },
            Err(OutputUnavailable::ScreenOperation) => Output::ScreenOperation,
            Err(OutputUnavailable::ByteLimit) => Output::ByteLimit,
        };
        let output_source = record.output_source.as_ref().map(|source| {
            let mut source = RetainedOutputSource::from(source);
            source.identity =
                stream.map(|stream| OutputSourceIdentity::new(stream, record.sequence));
            source
        });
        let output_annotations = record
            .output
            .as_ref()
            .ok()
            .and_then(|text| OutputAnnotations::new(text, record.output_annotations.as_ref()?));
        let mut block = Self {
            sequence: sequence.to_string(),
            execution_id: record.execution_id.clone(),
            shell_session: record.shell_session.clone(),
            shell_sequence: record.shell_sequence.map(|sequence| sequence.to_string()),
            command: record.command.clone(),
            cwd: record.cwd.clone(),
            exit: record.outcome.reported_exit_code(),
            provenance: record.provenance.schema_name().to_owned(),
            output_source,
            output,
            output_annotations,
            revision: String::new(),
        };
        // Include every retained field, including capture availability.
        // The empty revision is omitted from this stable wire projection.
        block.revision = block.digest_without_revision();
        block
    }

    fn digest_without_revision(&self) -> String {
        debug_assert!(self.revision.is_empty());
        let wire = serde_json::to_vec(self).expect("command block projection is JSON");
        format!("{:x}", Sha256::digest(wire))
    }

    fn valid_revision(&self) -> bool {
        let mut record = self.clone();
        let revision = std::mem::take(&mut record.revision);
        revision == record.digest_without_revision()
    }

    pub fn captured_output(&self) -> Option<&str> {
        match &self.output {
            Output::Text { text } => Some(text),
            Output::ScreenOperation | Output::ByteLimit => None,
        }
    }
}

fn canonical_number(value: &str) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|number| number.to_string() == value)
}

fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Immutable Aether records and the next sequence visible in a restored pane.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPaneHistory {
    next_sequence: u64,
    records: Vec<RetainedBlock>,
}

impl CompletedPaneHistory {
    pub fn cursor(&self) -> u64 {
        self.next_sequence.saturating_sub(1)
    }

    pub fn has_output_annotations(&self) -> bool {
        self.records
            .iter()
            .any(|record| record.output_annotations.is_some())
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut previous = self.next_sequence;
        let mut executions = std::collections::HashSet::new();
        if previous == 0 || self.records.len() > MAX_BLOCKS {
            return Err("Invalid completed command history.".into());
        }
        for block in &self.records {
            let sequence = canonical_number(&block.sequence)
                .filter(|&sequence| sequence > 0 && sequence < previous);
            let identity = block
                .output_source
                .as_ref()
                .filter(|source| source.valid())
                .and_then(|source| source.identity.as_ref());
            let valid = sequence.is_some()
                && identity.is_some_and(|identity| executions.insert(&identity.execution_id))
                && block.valid_checkpoint_fields();
            if !valid {
                return Err("Invalid completed command history record.".into());
            }
            previous = sequence.unwrap();
        }
        Ok(())
    }
}

impl RetainedOutputSource {
    fn valid(&self) -> bool {
        let range = canonical_number(&self.byte_start).zip(canonical_number(&self.byte_end));
        let events = canonical_number(&self.start_event).zip(canonical_number(&self.end_event));
        let size = |(cols, rows): (usize, usize)| {
            cols > 0 && rows > 0 && cols <= usize::from(u16::MAX) && rows <= usize::from(u16::MAX)
        };
        range.is_some_and(|(start, end)| start <= end)
            && events.is_some_and(|(start, end)| start > 0 && start < end)
            && size(self.start_size)
            && size(self.end_size)
            && (self.resized || self.start_size == self.end_size)
            && self.identity.as_ref().is_some_and(|identity| {
                lowercase_hex(&identity.stream_id, 32) && lowercase_hex(&identity.execution_id, 64)
            })
    }
}

impl RetainedBlock {
    fn valid_checkpoint_fields(&self) -> bool {
        let invisible = |character: char| character.is_whitespace() && character != ' ';
        !self.execution_id.is_empty()
            && self.execution_id.len() <= shell_markers::MAX_OSC133_ID_BYTES
            && !self.execution_id.chars().any(char::is_control)
            && !self.command.trim().is_empty()
            && shell_markers::valid_command(
                &self.command,
                shell_markers::MAX_OSC133_COMMAND_BYTES,
                invisible,
            )
            && self
                .cwd
                .as_deref()
                .is_none_or(|cwd| shell_markers::valid_cwd(cwd, invisible))
            && self
                .shell_session
                .as_deref()
                .is_none_or(shell_markers::is_valid_jsh_session_id)
            && self
                .shell_sequence
                .as_deref()
                .is_none_or(|value| canonical_number(value).is_some())
            && self.provenance == "shell_reported"
            && self.captured_output().is_none_or(|text| {
                text.len() <= MAX_OUTPUT_BYTES
                    && !text.chars().any(|character| {
                        character.is_control() && !matches!(character, '\n' | '\t')
                    })
            })
            && self.output_annotations.as_ref().is_none_or(|annotations| {
                self.captured_output()
                    .is_some_and(|text| annotations.clone().restore(text).is_some())
            })
            && self.valid_revision()
    }
}

#[derive(Default)]
pub(super) struct RestoredCommands {
    pane: Option<String>,
    offset: u64,
    records: Vec<RetainedBlock>,
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
    pub fn restore_completed_pane(
        &mut self,
        epoch: u32,
        pane: String,
        history: Option<CompletedPaneHistory>,
    ) -> Result<(), String> {
        self.check(epoch)?;
        let live = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .completed_command_blocks();
        if !self.local_desktop
            || live.is_none_or(|records| !records.is_empty())
            || self.restored_commands.pane.is_some()
        {
            return Err("Completed history requires a fresh integrated local shell.".into());
        }
        let (offset, records) = match history {
            Some(history) => {
                history.validate()?;
                (history.cursor(), history.records)
            }
            None => (0, Vec::new()),
        };
        self.restored_commands = RestoredCommands {
            pane: Some(pane),
            offset,
            records,
        };
        Ok(())
    }

    pub fn completed_pane_id(&self) -> Option<&str> {
        self.restored_commands.pane.as_deref()
    }

    pub fn completed_pane_cursor(&self, epoch: u32) -> Result<Option<Counter>, String> {
        self.check(epoch)?;
        let live = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .completed_command_blocks();
        let live = live
            .and_then(|records| records.back())
            .map_or(0, |record| record.sequence);
        Ok(live
            .checked_add(self.restored_commands.offset)
            .and_then(Counter::new))
    }

    pub fn completed_pane_history(&self, epoch: u32) -> Result<CompletedPaneHistory, String> {
        let next_sequence = self
            .completed_pane_cursor(epoch)?
            .map_or(0, Counter::get)
            .checked_add(1)
            .ok_or("Completed history sequence exhausted.")?;
        let records = self
            .command_blocks(epoch)?
            .blocks
            .into_iter()
            .filter(|block| {
                block
                    .output_source
                    .as_ref()
                    .is_some_and(|source| source.identity.is_some())
            })
            .collect();
        let history = CompletedPaneHistory {
            next_sequence,
            records,
        };
        history.validate()?;
        Ok(history)
    }

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
        let snapshot = self.command_blocks(epoch)?;
        let Some(command) = snapshot
            .blocks
            .into_iter()
            .find(|block| choice.matches(block) && !snapshot.restored.contains(&block.sequence))
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
        let stream = self.command_source_stream.as_deref();
        let offset = self.restored_commands.offset;
        let mut blocks: Vec<_> = records
            .into_iter()
            .flat_map(|blocks| {
                blocks.iter().rev().filter_map(move |record| {
                    offset
                        .checked_add(record.sequence)
                        .map(|sequence| RetainedBlock::project(record, sequence, stream))
                })
            })
            .take(MAX_BLOCKS)
            .collect();
        let live_count = blocks.len();
        blocks.extend(
            self.restored_commands
                .records
                .iter()
                .take(MAX_BLOCKS.saturating_sub(live_count))
                .cloned(),
        );
        let restored = blocks[live_count..]
            .iter()
            .map(|block| block.sequence.clone())
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
            restored,
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
            output_annotations: None,
            output_source: None,
        })
    }

    #[test]
    fn captured_sources_preserve_64_bit_identity_in_the_wire_projection() {
        let mut terminal = jterm_engine::terminal::Terminal::new(24, 80).unwrap();
        terminal.observe_command_blocks();
        let start = b"\x1b]133;A\x07$ \x1b]133;B\x07\x1b]133;C;id=run;command=printf%20fixture\x07";
        let output = b"\x1b[38;2;31;171;193mcaptured\x1b[0m\r\n";
        let end = b"\x1b]133;D;0;id=run\x1b\\";
        for bytes in [start.as_slice(), output, end] {
            terminal.feed(bytes).unwrap();
        }
        let mut record = terminal.completed_command_blocks().unwrap()[0].clone();
        let wire =
            |record: &CommandBlock| serde_json::to_value(RetainedBlock::from(record)).unwrap();
        let first = wire(&record);
        assert_eq!(first["execution_id"], "run");
        assert_eq!(first["output"]["text"], "captured\n");
        assert_eq!(
            first["output_annotations"]["spans"][0]["bytes"],
            serde_json::json!({"start": 0, "end": 8})
        );
        assert_eq!(
            first["output_annotations"]["spans"][0]["colors"][0],
            0x02_1f_ab_c1
        );
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

        let stream = "3625146840963580e7df3e478ad06731";
        let identified = RetainedBlock::project(&record, record.sequence, Some(stream));
        let repeated = RetainedBlock::project(&record, record.sequence, Some(stream));
        assert_eq!(identified.revision, repeated.revision);
        let wire = serde_json::to_value(&identified).unwrap();
        assert_eq!(wire["output_source"]["stream_id"], stream);
        assert_eq!(
            wire["output_source"]["execution_id"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(wire["execution_id"], "run");
        let other_stream = RetainedBlock::project(
            &record,
            record.sequence,
            Some("18ebf80f2977a1f4c5f36ba4812e5d63"),
        );
        assert_ne!(other_stream.revision, identified.revision);
        record.sequence += 1;
        let same_stream = RetainedBlock::project(&record, record.sequence, Some(stream));
        assert_ne!(
            identified
                .output_source
                .as_ref()
                .unwrap()
                .identity
                .as_ref()
                .unwrap()
                .execution_id,
            same_stream
                .output_source
                .unwrap()
                .identity
                .unwrap()
                .execution_id,
        );
        let checkpoint = CompletedPaneHistory {
            next_sequence: 2,
            records: vec![identified],
        };
        let bytes = serde_json::to_vec(&checkpoint).unwrap();
        let mut recovered: CompletedPaneHistory = serde_json::from_slice(&bytes).unwrap();
        recovered.validate().unwrap();
        assert_eq!(recovered.cursor(), 1);
        assert_eq!(serde_json::to_value(&recovered.records[0]).unwrap(), wire);
        recovered.records[0].command.push('x');
        assert!(recovered.validate().is_err());
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
            restored: Vec::new(),
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
