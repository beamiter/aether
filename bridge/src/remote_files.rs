//! Bounded file operations on a separate OpenSSH channel. The helper pins the
//! chosen directory for this connection. No operation consults a PTY or local preview.
use crate::{
    file_reference::*,
    terminal_transport::{read_record, ssh::Relay, write_record, Reply, Request, VERSION},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions, ReadDir},
    io::{self, BufRead, Read, Write},
    iter::Peekable,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};

// Base64 file body plus bounded path, cursor and JSON framing. A full directory
// of escaped names also fits (Linux directory entries are at most 255 bytes).
const RECORD_LIMIT: usize = MAX_FILE_TRANSFER_BYTES.div_ceil(3) * 4 + 32 * 1024;

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && !name.contains('/')
        && !name.chars().any(char::is_control)
}

pub fn child_path(directory: &str, name: &str) -> Result<String, String> {
    if !valid_name(name) {
        return Err("Choose a single file or directory name.".into());
    }
    let path = if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    };
    validate_relative(&path)?;
    Ok(path)
}

fn validate_relative(path: &str) -> Result<(), String> {
    if path.len() > MAX_FILE_REFERENCE_PATH_BYTES
        || (!path.is_empty() && !path.split('/').all(valid_name))
    {
        return Err("Choose a path inside the selected SSH project.".into());
    }
    Ok(())
}

pub fn file_version(bytes: &[u8]) -> FileVersion {
    FileVersion {
        bytes: bytes.len() as u32,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}

pub fn decode_transfer(encoded: &str, limit: usize) -> Result<Vec<u8>, String> {
    if encoded.len() <= limit.div_ceil(3) * 4 {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid encoded file contents.")?;
        if bytes.len() <= limit {
            return Ok(bytes);
        }
    }
    Err("File exceeds the transfer limit.".into())
}

pub fn exchange(relay: &mut Relay, command: FileCommand) -> io::Result<Result<FileReply, String>> {
    relay.exchange(command, RECORD_LIMIT, RECORD_LIMIT)
}

fn descriptor_path(directory: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

fn directory_at(root: &File, path: &str) -> Result<File, String> {
    validate_relative(path)?;
    let mut directory = root.try_clone().map_err(|e| e.to_string())?;
    if !path.is_empty() {
        for name in path.split('/') {
            directory = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(descriptor_path(&directory).join(name))
                .map_err(|e| format!("Could not open SSH directory: {e}"))?;
        }
    }
    Ok(directory)
}

fn parent_at(root: &File, path: &str) -> Result<(File, PathBuf), String> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    validate_relative(path)?;
    if !valid_name(name) {
        return Err("Choose a file or subdirectory of the SSH project.".into());
    }
    let directory = directory_at(root, parent)?;
    let target = descriptor_path(&directory).join(name);
    Ok((directory, target))
}

pub fn read_regular_file(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("Could not open file: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("Select a regular file within the byte limit.".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("File exceeds the byte limit.".into());
    }
    Ok(bytes)
}

struct Listing {
    path: String,
    entries: Peekable<ReadDir>,
    cursor: Option<String>,
}

impl Listing {
    fn open(root: &File, path: String) -> Result<Self, String> {
        let directory = directory_at(root, &path)?;
        let entries = fs::read_dir(descriptor_path(&directory))
            .map_err(|e| e.to_string())?
            .peekable();
        Ok(Self {
            path,
            entries,
            cursor: None,
        })
    }

    fn page(&mut self) -> Result<FileReply, String> {
        let mut entries = Vec::new();
        for entry in self.entries.by_ref().take(MAX_DIRECTORY_ENTRIES) {
            let entry = entry.map_err(|e| e.to_string())?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !valid_name(&name) {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let kind = if kind.is_dir() {
                EntryKind::Directory
            } else if kind.is_file() {
                EntryKind::File
            } else {
                continue;
            };
            entries.push(DirectoryEntry { name, kind });
        }
        entries.sort_by(|a, b| {
            (a.kind == EntryKind::File, &a.name).cmp(&(b.kind == EntryKind::File, &b.name))
        });
        self.cursor = self
            .entries
            .peek()
            .map(|_| crate::semantic::session_id())
            .transpose()?;
        Ok(FileReply::Directory {
            page: DirectoryPage {
                path: self.path.clone(),
                entries,
                next: self.cursor.clone(),
            },
        })
    }
}

#[derive(Default)]
struct Project {
    root: Option<File>,
    listing: Option<Listing>,
}

impl Project {
    fn list(&mut self, root: &File, path: String) -> Result<FileReply, String> {
        let mut listing = Listing::open(root, path)?;
        let result = listing.page()?;
        self.listing = Some(listing);
        Ok(result)
    }

    fn execute(&mut self, command: FileCommand) -> Result<FileReply, String> {
        if let FileCommand::Open { root } = command {
            if self.root.is_some() {
                return Err("An SSH project is already open.".into());
            }
            let relative = root
                .strip_prefix('/')
                .ok_or("Choose an absolute SSH project directory.")?;
            let directory = directory_at(&File::open("/").map_err(|e| e.to_string())?, relative)?;
            let result = self.list(&directory, String::new())?;
            self.root = Some(directory);
            return Ok(result);
        }
        let root = self.root.as_ref().ok_or("Open an SSH project first.")?;
        match command {
            FileCommand::Open { .. } => unreachable!(),
            FileCommand::List { path } => {
                let root = root.try_clone().map_err(|e| e.to_string())?;
                self.list(&root, path)
            }
            FileCommand::More { path, cursor } => {
                let listing = self
                    .listing
                    .as_mut()
                    .filter(|l| l.path == path && l.cursor.as_ref() == Some(&cursor))
                    .ok_or("SSH directory page changed. Refresh the folder.")?;
                listing.cursor = None;
                listing.page()
            }
            FileCommand::Read { path, preview } => {
                let (_parent, target) = parent_at(root, &path)?;
                let limit = if preview {
                    MAX_TEXT_PREVIEW_BYTES
                } else {
                    MAX_FILE_TRANSFER_BYTES
                };
                let bytes = read_regular_file(&target, limit)?;
                Ok(FileReply::Contents {
                    path,
                    version: file_version(&bytes),
                    base64: STANDARD.encode(bytes),
                })
            }
            FileCommand::Upload { path, base64 } => {
                let bytes = decode_transfer(&base64, MAX_FILE_TRANSFER_BYTES)?;
                let (parent, target) = parent_at(root, &path)?;
                // Stage on the same filesystem. Publication is no-replace even
                // when another SSH client creates the destination concurrently.
                let mut temp = tempfile::NamedTempFile::new_in(descriptor_path(&parent))
                    .map_err(|e| e.to_string())?;
                temp.write_all(&bytes)
                    .and_then(|()| temp.as_file().sync_all())
                    .map_err(|e| e.to_string())?;
                temp.persist_noclobber(target)
                    .map_err(|e| format!("Could not create new SSH file: {}", e.error))?;
                Ok(FileReply::Uploaded {
                    path,
                    version: file_version(&bytes),
                })
            }
            FileCommand::CreateDirectory { path } => {
                let (_parent, target) = parent_at(root, &path)?;
                fs::create_dir(target)
                    .map_err(|e| format!("Could not create SSH directory: {e}"))?;
                Ok(FileReply::CreatedDirectory { path })
            }
        }
    }
}

pub fn serve(mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    let mut project = Project::default();
    while let Some(request) = read_record::<Request<FileCommand>>(&mut input, RECORD_LIMIT)? {
        let result = if request.protocol == VERSION && request.request != 0 {
            project.execute(request.command)
        } else {
            Err("Unsupported SSH Files request.".into())
        };
        write_record(
            &mut output,
            &Reply {
                protocol: VERSION,
                request: request.request,
                result,
            },
            RECORD_LIMIT,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn open(root: &Path) -> Project {
        let mut project = Project::default();
        project
            .execute(FileCommand::Open {
                root: root.to_str().unwrap().to_owned(),
            })
            .unwrap();
        project
    }

    fn read(project: &mut Project, path: &str, preview: bool) -> Result<FileReply, String> {
        project.execute(FileCommand::Read {
            path: path.into(),
            preview,
        })
    }

    #[test]
    fn pinned_root_confines_navigation_and_refuses_replaced_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/note"), b"SSH version").unwrap();
        let outside = temp.path().join("elsewhere");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("note"), b"different source").unwrap();
        let mut project = open(&root);
        let Opened { version, bytes } = contents(read(&mut project, "dir/note", true).unwrap());
        assert_eq!(bytes, b"SSH version");
        assert_eq!(version, file_version(&bytes));
        for path in ["/note", "../elsewhere/note", "dir//note", "dir/../dir/note"] {
            assert!(read(&mut project, path, true).is_err());
        }
        fs::rename(root.join("dir"), root.join("original")).unwrap();
        symlink(&outside, root.join("dir")).unwrap();
        symlink(outside.join("note"), root.join("link")).unwrap();
        assert!(read(&mut project, "dir/note", true).is_err());
        assert!(read(&mut project, "link", true).is_err());
        fs::rename(&root, temp.path().join("moved")).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(read(&mut project, "note", true).is_err());
        assert_eq!(
            contents(read(&mut project, "original/note", true).unwrap()).bytes,
            bytes
        );
    }

    struct Opened {
        version: FileVersion,
        bytes: Vec<u8>,
    }
    fn contents(reply: FileReply) -> Opened {
        let FileReply::Contents {
            version, base64, ..
        } = reply
        else {
            panic!("expected file contents")
        };
        Opened {
            version,
            bytes: STANDARD.decode(base64).unwrap(),
        }
    }

    #[test]
    fn bounded_binary_upload_is_no_replace_and_preview_has_a_separate_limit() {
        let temp = tempfile::tempdir().unwrap();
        let mut project = open(temp.path());
        let bytes = vec![0xff; MAX_FILE_TRANSFER_BYTES];
        let write = || FileCommand::Upload {
            path: "image.bin".into(),
            base64: STANDARD.encode(&bytes),
        };
        assert!(matches!(
            project.execute(write()),
            Ok(FileReply::Uploaded { .. })
        ));
        assert!(project
            .execute(FileCommand::Upload {
                path: "image.bin".into(),
                base64: "ZXZpbA==".into()
            })
            .is_err());
        assert_eq!(
            contents(read(&mut project, "image.bin", false).unwrap()).bytes,
            bytes
        );
        assert!(read(&mut project, "image.bin", true).is_err());
        assert!(project
            .execute(FileCommand::Upload {
                path: "too-large".into(),
                base64: STANDARD.encode(vec![0; MAX_FILE_TRANSFER_BYTES + 1])
            })
            .is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn paging_cursor_belongs_to_one_directory_read() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..MAX_DIRECTORY_ENTRIES + 2 {
            File::create(temp.path().join(format!("file-{i}"))).unwrap();
        }
        let mut project = open(temp.path());
        let page = |result: FileReply| match result {
            FileReply::Directory { page } => page,
            _ => panic!("expected directory"),
        };
        let first = page(
            project
                .execute(FileCommand::List {
                    path: String::new(),
                })
                .unwrap(),
        );
        let next = first.next.unwrap();
        let more = || FileCommand::More {
            path: String::new(),
            cursor: next.clone(),
        };
        assert_eq!(first.entries.len(), MAX_DIRECTORY_ENTRIES);
        let second = page(project.execute(more()).unwrap());
        assert_eq!(second.entries.len(), 2);
        assert!(second.next.is_none());
        assert!(second
            .entries
            .iter()
            .all(|entry| !first.entries.iter().any(|old| old.name == entry.name)));
        assert!(project.execute(more()).is_err());
        project
            .execute(FileCommand::List {
                path: String::new(),
            })
            .unwrap();
        assert!(project.execute(more()).is_err());
    }
}
