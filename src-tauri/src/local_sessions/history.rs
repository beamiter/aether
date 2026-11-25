//! Durable, read-only command cards for explicitly restorable local panes.
use jterm_core::snapshot_file::{
    ensure_private_directory, read_bounded_private, write_atomic_private,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, Thread},
    time::{Duration, Instant},
};
use tauri_terminal_frame_bridge::live::{CompletedPaneHistory, LiveBridge};

const VERSION: u32 = 1;
const MAX_CHECKPOINT_BYTES: u64 = 2 * 1024 * 1024;
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct HistoryRequest {
    pub(super) pane: Option<String>,
    directory: PathBuf,
}

impl HistoryRequest {
    pub(crate) fn new(
        integrated: bool,
        pane: Option<String>,
        data_dir: Option<PathBuf>,
    ) -> Result<Option<Self>, String> {
        if pane.as_ref().is_some_and(|pane| {
            !integrated
                || pane.len() != 32
                || !pane
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        }) {
            return Err("Invalid completed command pane identity.".into());
        }
        if !integrated || !jterm_core::execution_journal::output_capture_enabled() {
            return Ok(None);
        }
        let Some(data_dir) = data_dir else {
            return if pane.is_some() {
                Err("Completed command history directory is unavailable.".into())
            } else {
                Ok(None)
            };
        };
        Ok(Some(Self {
            pane,
            directory: data_dir.join("completed-command-panes"),
        }))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    pane: String,
    history: CompletedPaneHistory,
}

struct Archive {
    path: PathBuf,
    pane: String,
    _lease: File,
    closed: AtomicBool,
}

impl Archive {
    fn lock(request: HistoryRequest) -> io::Result<Self> {
        let pane = request
            .pane
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let directory = request.directory;
        ensure_private_directory(&directory)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(format!("{pane}.lock")))?;
        let info = lease.metadata()?;
        // SAFETY: geteuid reads the current process's effective user identity.
        if !info.is_file()
            || info.nlink() != 1
            || info.mode() & 0o077 != 0
            || info.uid() != unsafe { libc::geteuid() }
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid pane history lock",
            ));
        }
        lease.try_lock()?;
        Ok(Self {
            path: directory.join(format!("{pane}.json")),
            pane,
            _lease: lease,
            closed: AtomicBool::new(false),
        })
    }

    fn load(&self) -> io::Result<Option<CompletedPaneHistory>> {
        let bytes = match read_bounded_private(&self.path, MAX_CHECKPOINT_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let checkpoint: Checkpoint = serde_json::from_str(&bytes)?;
        if checkpoint.version != VERSION || checkpoint.pane != self.pane {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incompatible pane history",
            ));
        }
        checkpoint
            .history
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(checkpoint.history))
    }

    fn save(&self, history: CompletedPaneHistory) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let checkpoint = Checkpoint {
            version: VERSION,
            pane: self.pane.clone(),
            history,
        };
        let bytes = serde_json::to_vec(&checkpoint)?;
        if bytes.len() as u64 > MAX_CHECKPOINT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pane checkpoint exceeds size limit",
            ));
        }
        let result = write_atomic_private(&self.path, &bytes);
        if self.closed.load(Ordering::SeqCst) {
            self.remove();
        }
        result
    }

    fn remove(&self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!("Could not remove closed pane history: {error}");
            }
        }
    }
}

struct Write {
    archive: Arc<Archive>,
    history: CompletedPaneHistory,
    finished: mpsc::Sender<io::Result<()>>,
}

pub(super) struct HistoryWriter(SyncSender<Write>);

impl HistoryWriter {
    pub(super) fn start(wake: Thread) -> Result<Self, String> {
        let (send, receive) = mpsc::sync_channel::<Write>(2);
        thread::Builder::new()
            .name("aether-completed-history".into())
            .spawn(move || {
                for job in receive {
                    let _ = job.finished.send(job.archive.save(job.history));
                    wake.unpark();
                }
            })
            .map_err(|error| format!("Completed command history writer is unavailable: {error}"))?;
        Ok(Self(send))
    }
}

pub(super) struct PaneHistory {
    archive: Arc<Archive>,
    saved: u64,
    pending: Option<(u64, Receiver<io::Result<()>>)>,
    retry_after: Instant,
}

impl PaneHistory {
    pub(super) fn open(
        request: HistoryRequest,
    ) -> Result<(Self, Option<CompletedPaneHistory>), String> {
        let archive = Archive::lock(request)
            .map_err(|error| format!("Cannot own completed pane history: {error}"))?;
        let restored = archive
            .load()
            .map_err(|error| format!("Cannot read completed pane history: {error}"))?;
        let saved = restored.as_ref().map_or(0, CompletedPaneHistory::cursor);
        Ok((
            Self {
                archive: Arc::new(archive),
                saved,
                pending: None,
                retry_after: Instant::now(),
            },
            restored,
        ))
    }

    pub(super) fn restore(
        &self,
        bridge: &mut LiveBridge,
        epoch: u32,
        checkpoint: Option<CompletedPaneHistory>,
    ) -> Result<(), String> {
        bridge.restore_completed_pane(epoch, self.archive.pane.clone(), checkpoint)
    }

    pub(super) fn update(
        &mut self,
        writer: &HistoryWriter,
        bridge: &LiveBridge,
        epoch: u32,
    ) -> bool {
        if let Some((sequence, reply)) = &self.pending {
            match reply.try_recv() {
                Ok(Ok(())) => self.saved = *sequence,
                Ok(Err(error)) => {
                    eprintln!("Could not save completed pane history: {error}");
                    self.retry_after = Instant::now() + RETRY_INTERVAL;
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    self.retry_after = Instant::now() + RETRY_INTERVAL
                }
            }
            self.pending = None;
        }
        let Some(cursor) = bridge.completed_pane_cursor(epoch).ok().flatten() else {
            return false;
        };
        if cursor.get() <= self.saved {
            return false;
        }
        if Instant::now() >= self.retry_after {
            match bridge.completed_pane_history(epoch) {
                Ok(history) => {
                    let (finished, reply) = mpsc::channel();
                    let sequence = history.cursor();
                    if writer
                        .0
                        .try_send(Write {
                            archive: self.archive.clone(),
                            history,
                            finished,
                        })
                        .is_ok()
                    {
                        self.pending = Some((sequence, reply));
                    }
                }
                Err(error) => {
                    eprintln!("Could not capture completed pane history: {error}");
                    self.retry_after = Instant::now() + RETRY_INTERVAL;
                }
            }
        }
        true
    }

    pub(super) fn close(&self) {
        self.archive.closed.store(true, Ordering::SeqCst);
        self.archive.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_has_one_writer_and_incompatible_checkpoints_survive_a_failed_load() {
        let root = tempfile::tempdir().unwrap();
        let pane = "1".repeat(32);
        let request = || HistoryRequest {
            pane: Some(pane.clone()),
            directory: root.path().join("panes"),
        };
        let archive = Archive::lock(request()).unwrap();
        assert!(Archive::lock(request()).is_err());
        assert!(archive.load().unwrap().is_none());
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version": VERSION + 1, "pane": pane,
            "history": { "next_sequence": 1, "records": [] }
        }))
        .unwrap();
        write_atomic_private(&archive.path, &bytes).unwrap();
        assert_eq!(
            archive.load().err().unwrap().kind(),
            io::ErrorKind::InvalidData
        );
        let path = archive.path.clone();
        drop(archive);
        assert!(Archive::lock(request()).is_ok());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
