//! The main window's explicit SSH Files connection, independent of terminal transport.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri_terminal_frame_bridge::{
    file_reference::*,
    remote_files::{child_path, decode_transfer, exchange, file_version, valid_name},
    terminal_transport::ssh::{Endpoint, Relay},
};
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct Files {
    connection: Option<Connection>,
}

struct Connection {
    relay: Option<Relay>,
    current: Folder,
}

#[derive(Clone, Serialize)]
pub(crate) struct Folder {
    project: SshProject,
    page_id: Uuid,
    directory: DirectoryPage,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    view: String,
    page_id: Uuid,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Navigation {
    Refresh,
    Parent,
    Next,
    Enter { name: String },
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    Inspect,
    Open {
        host: String,
        root: String,
    },
    Close {
        view: String,
    },
    Browse {
        selection: Selection,
        navigation: Navigation,
    },
    Read {
        selection: Selection,
        name: String,
        preview: bool,
        expected: Option<FileVersion>,
    },
    Upload {
        selection: Selection,
        name: String,
        base64: String,
    },
    CreateDirectory {
        selection: Selection,
        name: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ResultView {
    Closed,
    Folder {
        folder: Folder,
    },
    Preview {
        preview: FilePreview,
    },
    Download {
        reference: FileReference,
        base64: String,
    },
    Uploaded {
        reference: FileReference,
    },
    CreatedDirectory {
        project: SshProject,
        path: String,
    },
}

#[tauri::command]
pub(crate) async fn ssh_files<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    action: Action,
    files: tauri::State<'_, Arc<Mutex<Files>>>,
) -> Result<ResultView, String> {
    super::require_terminal_window(&window)?;
    super::run_bridge(files.inner().clone(), move |files| files.execute(action)).await
}

impl Files {
    fn execute(&mut self, action: Action) -> Result<ResultView, String> {
        match action {
            Action::Inspect => Ok(self
                .connection
                .as_ref()
                .map_or(ResultView::Closed, |c| c.folder())),
            Action::Open { host, root } => {
                if self.connection.is_some() {
                    return Err("Close the current SSH project first.".into());
                }
                if !root.starts_with('/') || root.len() > MAX_FILE_REFERENCE_PATH_BYTES {
                    return Err("Choose an absolute SSH project directory.".into());
                }
                let host = host.trim().to_owned();
                let root = root.trim_end_matches('/');
                let root = if root.is_empty() { "/" } else { root }.to_owned();
                let helper = std::env::var_os("AETHER_SSH_TERMINAL_HELPER").map(PathBuf::from);
                let mut relay = Endpoint::files(&host, helper.as_deref())?
                    .connect()
                    .map_err(|e| e.to_string())?;
                let reply = exchange(&mut relay, FileCommand::Open { root: root.clone() })
                    .map_err(|_| "Could not open SSH Files. Check the host key, login and installed file helper.")??;
                let directory = checked_page(reply, "")?;
                let project = SshProject {
                    host,
                    root,
                    view: Uuid::new_v4().to_string(),
                };
                let connection = Connection {
                    relay: Some(relay),
                    current: Folder {
                        project,
                        page_id: Uuid::new_v4(),
                        directory,
                    },
                };
                let result = connection.folder();
                self.connection = Some(connection);
                Ok(result)
            }
            Action::Close { view } => {
                if self
                    .connection
                    .as_ref()
                    .is_some_and(|c| c.current.project.view != view)
                {
                    return Err("The SSH project changed.".into());
                }
                self.connection = None;
                Ok(ResultView::Closed)
            }
            Action::Browse {
                selection,
                navigation,
            } => self.selected(&selection)?.browse(navigation),
            Action::Read {
                selection,
                name,
                preview,
                expected,
            } => self.selected(&selection)?.read(name, preview, expected),
            Action::Upload {
                selection,
                name,
                base64,
            } => self.selected(&selection)?.upload(name, base64),
            Action::CreateDirectory { selection, name } => {
                let c = self.selected(&selection)?;
                let path = child_path(&c.current.directory.path, &name)?;
                match c.call(FileCommand::CreateDirectory { path: path.clone() })? {
                    FileReply::CreatedDirectory { path: confirmed } if confirmed == path => {
                        Ok(ResultView::CreatedDirectory {
                            project: c.current.project.clone(),
                            path,
                        })
                    }
                    _ => {
                        Err("SSH directory creation was not confirmed. Refresh the folder.".into())
                    }
                }
            }
        }
    }

    fn selected(&mut self, selection: &Selection) -> Result<&mut Connection, String> {
        self.connection
            .as_mut()
            .filter(|c| {
                c.current.project.view == selection.view && c.current.page_id == selection.page_id
            })
            .ok_or_else(|| "The SSH folder selection changed. Refresh the folder.".into())
    }
}

fn checked_page(reply: FileReply, path: &str) -> Result<DirectoryPage, String> {
    let FileReply::Directory { page } = reply else {
        return Err("Expected an SSH directory.".into());
    };
    let mut names = HashSet::new();
    if page.path != path
        || page.entries.len() > MAX_DIRECTORY_ENTRIES
        || page
            .entries
            .iter()
            .any(|e| !valid_name(&e.name) || !names.insert(&e.name))
        || page
            .next
            .as_ref()
            .is_some_and(|s| s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("Invalid SSH directory reply.".into());
    }
    Ok(page)
}

impl Connection {
    fn folder(&self) -> ResultView {
        ResultView::Folder {
            folder: self.current.clone(),
        }
    }
    fn call(&mut self, command: FileCommand) -> Result<FileReply, String> {
        let relay = self
            .relay
            .as_mut()
            .ok_or("SSH Files disconnected. Close the project and reopen it.")?;
        match exchange(relay, command) {
            Ok(result) => result,
            Err(_) => {
                self.relay = None;
                Err(
                    "SSH Files disconnected. Reopen the project to check the last operation."
                        .into(),
                )
            }
        }
    }
    fn entry(&self, name: &str, kind: EntryKind) -> Result<String, String> {
        let dir = &self.current.directory;
        if !dir.entries.iter().any(|e| e.kind == kind && e.name == name) {
            return Err("Select an entry in the current SSH folder.".into());
        }
        child_path(&dir.path, name)
    }
    fn browse(&mut self, navigation: Navigation) -> Result<ResultView, String> {
        let dir = &self.current.directory;
        let path = match &navigation {
            Navigation::Parent => dir.path.rsplit_once('/').map_or("", |(p, _)| p).to_owned(),
            Navigation::Enter { name } => self.entry(name, EntryKind::Directory)?,
            Navigation::Refresh | Navigation::Next => dir.path.clone(),
        };
        let command = if matches!(navigation, Navigation::Next) {
            FileCommand::More {
                path: path.clone(),
                cursor: dir
                    .next
                    .clone()
                    .ok_or("No further SSH directory entries.")?,
            }
        } else {
            FileCommand::List { path: path.clone() }
        };
        let page = checked_page(self.call(command)?, &path)?;
        self.current.directory = page;
        self.current.page_id = Uuid::new_v4();
        Ok(self.folder())
    }
    fn reference(&self, path: String, version: FileVersion) -> FileReference {
        FileReference::SshProject {
            project: self.current.project.clone(),
            path,
            version,
        }
    }
    fn read(
        &mut self,
        name: String,
        preview: bool,
        expected: Option<FileVersion>,
    ) -> Result<ResultView, String> {
        let path = self.entry(&name, EntryKind::File)?;
        let FileReply::Contents {
            path: confirmed,
            version,
            base64,
        } = self.call(FileCommand::Read {
            path: path.clone(),
            preview,
        })?
        else {
            return Err("Missing SSH file snapshot.".into());
        };
        let limit = if preview {
            MAX_TEXT_PREVIEW_BYTES
        } else {
            MAX_FILE_TRANSFER_BYTES
        };
        let bytes = decode_transfer(&base64, limit)?;
        if confirmed != path || file_version(&bytes) != version {
            return Err("SSH file snapshot identity or checksum did not match.".into());
        }
        if expected.as_ref().is_some_and(|old| *old != version) {
            return Err(
                "SSH file changed since preview. Preview it again to download that version.".into(),
            );
        }
        let reference = self.reference(path, version);
        if preview {
            let text = String::from_utf8(bytes)
                .ok()
                .filter(|s| !s.contains('\0'))
                .ok_or("Choose a UTF-8 text file for preview or download the original bytes.")?;
            Ok(ResultView::Preview {
                preview: FilePreview { reference, text },
            })
        } else {
            Ok(ResultView::Download { reference, base64 })
        }
    }
    fn upload(&mut self, name: String, base64: String) -> Result<ResultView, String> {
        let path = child_path(&self.current.directory.path, &name)?;
        let expected = file_version(&decode_transfer(&base64, MAX_FILE_TRANSFER_BYTES)?);
        match self.call(FileCommand::Upload {
            path: path.clone(),
            base64,
        })? {
            FileReply::Uploaded {
                path: confirmed,
                version,
            } if confirmed == path && version == expected => Ok(ResultView::Uploaded {
                reference: self.reference(path, version),
            }),
            _ => Err("SSH file creation was not confirmed. Refresh the folder.".into()),
        }
    }
}
