use jterm_core::snapshot_file::{
    ensure_private_directory, read_bounded_private, write_atomic_private,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri_terminal_frame_bridge::{
    semantic::{Attachment, SessionSelector},
    terminal_transport::{ssh::Endpoint, valid_identity},
};

const MAX_ENTRIES: usize = 16;
const MAX_BYTES: u64 = 16 * 1024;
const CATALOG_VERSION: u8 = 1;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedShell {
    pub id: String,
    pub destination: String,
    pub instance: String,
    pub target: SessionSelector,
    created_ms: u64,
}

impl SavedShell {
    pub fn new(endpoint: &Endpoint, instance: String, owner: &Attachment) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            destination: endpoint.destination().into(),
            instance,
            target: SessionSelector {
                session: owner.session.clone(),
                epoch: owner.epoch,
            },
            created_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    version: u8,
    shells: Vec<SavedShell>,
}

#[derive(Clone)]
pub(super) struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn new(app_data: PathBuf) -> Self {
        Self {
            directory: app_data.join("ssh-terminals"),
        }
    }

    fn path(&self) -> PathBuf {
        self.directory.join("sessions.json")
    }

    fn read(&self) -> Result<Catalog, String> {
        let source = match read_bounded_private(&self.path(), MAX_BYTES) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Catalog {
                    version: CATALOG_VERSION,
                    shells: Vec::new(),
                });
            }
            Err(_) => return Err("The saved SSH session directory could not be read.".into()),
        };
        let catalog: Catalog = serde_json::from_str(&source)
            .map_err(|_| "The saved SSH session directory is invalid.")?;
        let mut ids = HashSet::new();
        if catalog.version != CATALOG_VERSION
            || catalog.shells.len() > MAX_ENTRIES
            || catalog.shells.iter().any(|shell| {
                !valid_identity(&shell.id)
                    || !ids.insert(&shell.id)
                    || !valid_identity(&shell.instance)
                    || !valid_identity(&shell.target.session)
                    || shell.target.epoch == 0
                    || Endpoint::new(&shell.destination, None, None).is_err()
            })
        {
            return Err("The saved SSH session directory is invalid.".into());
        }
        Ok(catalog)
    }

    pub fn list(&self) -> Result<Value, String> {
        Ok(Value::Array(
            self.read()?
                .shells
                .iter()
                .rev()
                .map(|shell| {
                    json!({
                        "id": shell.id,
                        "destination": shell.destination,
                        "created_ms": shell.created_ms,
                    })
                })
                .collect(),
        ))
    }

    pub fn get(&self, id: &str) -> Result<SavedShell, String> {
        self.read()?
            .shells
            .into_iter()
            .find(|shell| shell.id == id)
            .ok_or_else(|| "That saved SSH shell is no longer in the directory.".into())
    }

    pub fn remember(&self, shell: SavedShell) -> Result<(), String> {
        self.edit(|catalog| {
            if catalog.shells.len() == MAX_ENTRIES {
                return Err("The saved SSH session directory is full.".into());
            }
            catalog.shells.push(shell);
            Ok(true)
        })
    }

    pub fn forget(&self, id: &str) -> Result<(), String> {
        self.edit(|catalog| {
            let before = catalog.shells.len();
            catalog.shells.retain(|shell| shell.id != id);
            Ok(catalog.shells.len() != before)
        })
    }

    fn edit(
        &self,
        operation: impl FnOnce(&mut Catalog) -> Result<bool, String>,
    ) -> Result<(), String> {
        let storage_error = "The saved SSH session directory could not be updated.";
        ensure_private_directory(&self.directory).map_err(|_| storage_error)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(self.directory.join("sessions.lock"))
            .map_err(|_| storage_error)?;
        let metadata = lock.metadata().map_err(|_| storage_error)?;
        // SAFETY: geteuid reads only the current process credential.
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(storage_error.into());
        }
        lock.try_lock()
            .map_err(|_| "Another Aether process is updating saved SSH sessions. Try again.")?;
        let result = (|| {
            let mut catalog = self.read()?;
            if operation(&mut catalog)? {
                let contents = serde_json::to_vec(&catalog).map_err(|_| storage_error)?;
                if contents.len() as u64 > MAX_BYTES {
                    return Err(storage_error.into());
                }
                write_atomic_private(&self.path(), &contents).map_err(|_| storage_error)?;
            }
            Ok(())
        })();
        // A PTY child can retain a forked copy until exec.
        let _ = lock.unlock();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(destination: &str) -> SavedShell {
        SavedShell::new(
            &Endpoint::new(destination, None, None).unwrap(),
            "c".repeat(32),
            &Attachment {
                session: "b".repeat(32),
                epoch: 2,
                generation: 9,
            },
        )
    }

    #[test]
    fn restart_loads_a_private_directory_and_lists_only_display_fields() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::new(home.path().into());
        let shell = shell("same-host");
        let id = shell.id.clone();
        assert_eq!(store.list().unwrap(), json!([]));
        store.remember(shell).unwrap();
        drop(store);

        let restarted = Store::new(home.path().into());
        assert_eq!(restarted.path().metadata().unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            restarted.directory.metadata().unwrap().mode() & 0o777,
            0o700
        );
        let display = restarted.list().unwrap();
        let fields = display[0].as_object().unwrap();
        assert_eq!(fields.len(), 3);
        assert_eq!(display[0]["id"], id);
        assert_eq!(display[0]["destination"], "same-host");
        let saved = restarted.get(&id).unwrap();
        assert_eq!(saved.target.session, "b".repeat(32));
        assert_eq!(saved.target.epoch, 2);
        assert_eq!(saved.instance, "c".repeat(32));
        restarted.forget(&id).unwrap();
        assert!(restarted.get(&id).is_err());
    }

    #[test]
    fn a_second_writer_keeps_existing_entries_and_bad_catalog_bytes() {
        let home = tempfile::tempdir().unwrap();
        let first = Store::new(home.path().into());
        let second = Store::new(home.path().into());
        first.remember(shell("first")).unwrap();
        second.remember(shell("second")).unwrap();
        assert_eq!(first.list().unwrap().as_array().unwrap().len(), 2);

        let lock = OpenOptions::new()
            .write(true)
            .open(first.directory.join("sessions.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        assert!(second.remember(shell("third")).is_err());
        lock.unlock().unwrap();
        drop(lock);
        assert_eq!(first.list().unwrap().as_array().unwrap().len(), 2);

        let corrupt = br#"{"version":2,"shells":[]}"#;
        write_atomic_private(&first.path(), corrupt).unwrap();
        assert_eq!(
            first.remember(shell("fourth")).unwrap_err(),
            "The saved SSH session directory is invalid."
        );
        assert_eq!(std::fs::read(first.path()).unwrap(), corrupt);
    }
}
