use super::LiveBridge;
use crate::semantic::Attachment;
use jterm_engine::command_blocks::{CommandBlock, OutputUnavailable};
use sha2::{Digest, Sha256};

#[derive(serde::Serialize)]
pub struct CommandBlockSnapshot {
    owner: Attachment,
    enabled: bool,
    blocks: Vec<RetainedBlock>,
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
    output: Output,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revision: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockSelection {
    pub sequence: String,
    pub revision: String,
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
                .position(|record| {
                    record.sequence == choice.sequence && record.revision == choice.revision
                })
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
            .find(|block| block.sequence == choice.sequence && block.revision == choice.revision)
            .map(|block| block.command)
        else {
            return Ok(Some(
                "The selected command revision has expired. Refresh the command list.",
            ));
        };
        self.paste(epoch, &command)?;
        Ok(None)
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
        Ok(CommandBlockSnapshot {
            owner,
            enabled: records.is_some(),
            blocks: records
                .into_iter()
                .flat_map(|blocks| blocks.iter().rev().map(RetainedBlock::from))
                .collect(),
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
        })
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
