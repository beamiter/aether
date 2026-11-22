use std::path::{Path, PathBuf};
use tauri_terminal_frame_bridge::{
    file_reference::{
        FileReference, FileVersion, MAX_FILE_REFERENCE_PATH_BYTES, MAX_TEXT_PREVIEW_BYTES,
    },
    remote_files::{file_version, read_regular_file, valid_name},
};

pub(super) struct Snapshot {
    suffix: PathBuf,
    version: FileVersion,
    pub(super) position: Option<(u64, Option<u64>)>,
}

impl Snapshot {
    pub(super) fn new(
        reference: FileReference,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<Self, String> {
        let FileReference::LocalSnapshot {
            directory,
            path,
            version,
        } = reference
        else {
            return Err("Choose a local preview to open in Vim9.".into());
        };
        if path.len() > MAX_FILE_REFERENCE_PATH_BYTES
            || !path.split('/').all(valid_name)
            || directory.as_ref().is_some_and(|name| !valid_name(name))
            || (directory.is_none() && path.contains('/'))
            || version.bytes as usize > MAX_TEXT_PREVIEW_BYTES
            || version.sha256.len() != 64
            || !version
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("Invalid local snapshot reference.".into());
        }
        if line
            .into_iter()
            .chain(column)
            .any(|n| n == 0 || n > i32::MAX as u32)
            || (line.is_none() && column.is_some())
        {
            return Err("Vim requires a positive line and byte column up to 2147483647.".into());
        }
        let suffix = directory.map(PathBuf::from).unwrap_or_default().join(path);
        Ok(Self {
            suffix,
            version,
            position: line.map(|line| (line.into(), column.map(u64::from))),
        })
    }

    pub(super) fn verify(&self, chosen: &Path) -> Result<(), String> {
        if !chosen.ends_with(&self.suffix) {
            return Err("The chosen local path does not match the selected preview.".into());
        }
        let bytes = read_regular_file(chosen, MAX_TEXT_PREVIEW_BYTES)?;
        if file_version(&bytes) != self.version {
            return Err(
                "The chosen file differs from the preview. Select a fresh local snapshot.".into(),
            );
        }
        Ok(())
    }
}
