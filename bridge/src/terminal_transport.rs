//! Private Linux PTY owner and versioned RPC records for OpenSSH stdio.
mod reply_forward;
mod session_table;
pub mod ssh;

use crate::{
    live::{HistorySnapshot, KeyEvent, LiveBridge, NativeKeyState},
    protocol_v4::{Counter, RENDITION_VERSION},
    semantic::{Attachment, Resume, Update},
    SSH_DENSE_FRAME_VERSION,
};
use jterm_engine::input::KeyboardLocks;
use reply_forward::{forward_identify_reply, forward_reply, forward_status_reply};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{value::RawValue, Value};
use session_table::SessionTable;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

pub const VERSION: u8 = 1;
// Ordinary relay IDs require a successor, leaving this ID available for an
// identity request that accepts capability metadata as well as the legacy ID.
const IDENTIFY_CAPABILITIES_REQUEST: u32 = u32::MAX;
pub const HOST_CHANGED: &str = "terminal host instance changed";
const MAX_REQUEST: usize = 128 * 1024;
const MAX_REPLY: usize = 16 * 1024 * 1024;
const MAX_INPUT: usize = 16 * 1024;
const MAX_SESSIONS: usize = 4;
const INACTIVITY: Duration = Duration::from_secs(10 * 60);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
const IO_RETRY: Duration = Duration::from_millis(25);
const FAILED_IO_RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request<C = Command> {
    pub protocol: u8,
    pub request: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub command: C,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum Command {
    Identify,
    Open {
        rows: usize,
        cols: usize,
    },
    AttachReadOnly {
        session: String,
        epoch: u32,
    },
    AttachReadOnlyPacked {
        session: String,
        epoch: u32,
    },
    Recover {
        owner: Attachment,
    },
    RecoverPacked {
        owner: Attachment,
    },
    Detach {
        owner: Attachment,
    },
    Poll {
        owner: Attachment,
    },
    PollWithStatus {
        owner: Attachment,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ack_revision: Option<u32>,
    },
    PollPacked {
        owner: Attachment,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ack_revision: Option<Counter>,
        snapshot: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame_version: Option<u8>,
    },
    Ack {
        owner: Attachment,
        revision: u32,
    },
    AckPacked {
        owner: Attachment,
        revision: Counter,
    },
    Status {
        owner: Attachment,
    },
    History {
        owner: Attachment,
        offset: usize,
    },
    Write {
        owner: Attachment,
        bytes: Vec<u8>,
    },
    Key {
        owner: Attachment,
        name: String,
        shift: bool,
        alt: bool,
        ctrl: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keypad: Option<crate::live::KeypadReport>,
    },
    EnableKeyboardEvents {
        owner: Attachment,
    },
    KeyEvent {
        owner: Attachment,
        event: KeyEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<KeyboardLayout>,
    },
    Mouse {
        owner: Attachment,
        report: crate::live::MouseReport,
    },
    Focus {
        owner: Attachment,
        focused: bool,
    },
    Text {
        owner: Attachment,
        text: String,
    },
    Paste {
        owner: Attachment,
        text: String,
    },
    Resize {
        owner: Attachment,
        rows: usize,
        cols: usize,
    },
    Close {
        owner: Attachment,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyboardLayout {
    base: Option<char>,
    caps_lock: bool,
    num_lock: bool,
}

impl From<NativeKeyState> for KeyboardLayout {
    fn from(state: NativeKeyState) -> Self {
        Self {
            base: state.base,
            caps_lock: state.locks.caps_lock,
            num_lock: state.locks.num_lock,
        }
    }
}

impl KeyboardLayout {
    fn into_native(self) -> Result<NativeKeyState, String> {
        if self.base.is_some_and(char::is_control) {
            return Err("invalid keyboard layout character".into());
        }
        Ok(NativeKeyState {
            base: self.base,
            locks: KeyboardLocks {
                caps_lock: self.caps_lock,
                num_lock: self.num_lock,
            },
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply<T = Value> {
    pub protocol: u8,
    pub request: u32,
    pub result: Result<T, String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PollReply<U> {
    pub update: U,
    pub status: Value,
}

pub type RawPollReply = PollReply<Box<RawValue>>;

#[derive(Debug, Deserialize, Serialize)]
pub struct PackedResume<F> {
    pub owner: Attachment,
    pub frame: F,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_allowed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyboard_events_enabled: Option<bool>,
}

pub type RawPackedResume = PackedResume<Box<RawValue>>;

// Keep cell-heavy replies typed until they are written to the socket.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum HostReply {
    Json(Value),
    Update(Option<Update>),
    PollWithStatus(PollReply<Option<Update>>),
    #[serde(skip_serializing)]
    PollPacked(PollReply<Option<String>>),
    #[serde(skip_serializing)]
    PackedResume(PackedResume<String>),
    Attached(Option<Resume>),
    Recovered {
        #[serde(flatten)]
        view: Resume,
        input_allowed: bool,
        keyboard_events_enabled: bool,
    },
    History(HistorySnapshot),
}

impl HostReply {
    fn json(value: impl Serialize) -> Result<Self, String> {
        reply_value(value).map(Self::Json)
    }
}

struct Host {
    identity: String,
    sessions: SessionTable<LiveBridge, MAX_SESSIONS>,
    wake: Arc<OutputWake>,
    retry_at: Option<Instant>,
}

struct OutputWake(OwnedFd);

impl OutputWake {
    fn new() -> io::Result<Self> {
        // SAFETY: eventfd returns a new descriptor on success.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: transfer the new, uniquely owned descriptor.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn notify(&self) {
        loop {
            // SAFETY: the descriptor is live for the duration of this call.
            if unsafe { libc::eventfd_write(self.0.as_raw_fd(), 1) } == 0 {
                return;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                // A full counter is already readable.
                return;
            }
        }
    }

    fn clear(&self) -> io::Result<()> {
        let mut count = 0;
        loop {
            // SAFETY: count is writable and the descriptor stays live.
            if unsafe { libc::eventfd_read(self.0.as_raw_fd(), &mut count) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(()),
                _ => return Err(error),
            }
        }
    }
}

pub fn valid_identity(identity: &str) -> bool {
    identity.len() == 32
        && identity
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn reply_value(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|_| "terminal reply serialization failed".into())
}

fn terminal_status(bridge: &LiveBridge, epoch: u32) -> Result<Value, String> {
    let mut status = reply_value(bridge.status(epoch)?)?;
    status["focus_reporting"] = Value::Bool(true);
    status["mouse_reporting"] = Value::Bool(true);
    status["keypad_reporting"] = Value::Bool(true);
    status["keyboard_events_supported"] = Value::Bool(true);
    status["poll_with_status"] = Value::Bool(true);
    status["packed_frame_version"] = RENDITION_VERSION.into();
    status["dense_frame_version"] = SSH_DENSE_FRAME_VERSION.into();
    if let Some(mouse) = bridge.mouse_view(epoch)? {
        status["mouse"] = reply_value(mouse)?;
    }
    if let Some(alternate) = bridge.alternate_scroll_view(epoch)? {
        status["alternate_scroll"] = reply_value(alternate)?;
    }
    Ok(status)
}

fn check_grid(rows: usize, cols: usize) -> Result<(), String> {
    if (1..=128).contains(&rows) && (1..=256).contains(&cols) {
        Ok(())
    } else {
        Err("terminal grid exceeds wire limit".into())
    }
}

fn check_input(length: usize) -> Result<(), String> {
    if length <= MAX_INPUT {
        Ok(())
    } else {
        Err("terminal input exceeds wire limit".into())
    }
}

impl Host {
    fn new() -> Result<Self, String> {
        Ok(Self {
            identity: crate::semantic::session_id()?,
            sessions: SessionTable::default(),
            wake: Arc::new(
                OutputWake::new().map_err(|error| format!("terminal owner wake: {error}"))?,
            ),
            retry_at: None,
        })
    }

    fn expire(&mut self, now: Instant) {
        self.sessions.expire(now, INACTIVITY);
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.retry_at
            .into_iter()
            .chain(self.sessions.next_expiry(INACTIVITY))
            .min()
    }

    fn pump(&mut self, now: Instant) {
        let mut retry = None;
        for (bridge, last_used) in self.sessions.activity_mut() {
            match bridge.pump_unpublished_with_activity() {
                Ok(activity) => {
                    if activity.output_bytes != 0 {
                        *last_used = now;
                    }
                    if activity.retry {
                        retry = Some(IO_RETRY);
                    }
                }
                Err(_) => {
                    retry.get_or_insert(FAILED_IO_RETRY);
                }
            }
        }
        self.retry_at = retry.map(|delay| Instant::now() + delay);
    }

    fn reply(&mut self, request: Request) -> Reply<HostReply> {
        let result = if request.protocol != VERSION || request.request == 0 {
            Err("unsupported terminal transport version or request ID".into())
        } else if (matches!(
            &request.command,
            Command::Focus { .. }
                | Command::Mouse { .. }
                | Command::EnableKeyboardEvents { .. }
                | Command::KeyEvent { .. }
                | Command::PollWithStatus { .. }
                | Command::PollPacked { .. }
                | Command::RecoverPacked { .. }
                | Command::AttachReadOnlyPacked { .. }
                | Command::AckPacked { .. }
        ) && request.host.as_ref() != Some(&self.identity))
            || request
                .host
                .as_ref()
                .is_some_and(|host| host != &self.identity)
        {
            Err(HOST_CHANGED.into())
        } else if request.request == IDENTIFY_CAPABILITIES_REQUEST
            && matches!(request.command, Command::Identify)
        {
            HostReply::json(serde_json::json!({
                "instance": &self.identity,
                "packed_snapshot_version": SSH_DENSE_FRAME_VERSION,
            }))
        } else {
            let resize = matches!(request.command, Command::Resize { .. });
            let result = self.dispatch(request.command);
            // RPCs can enqueue input or change geometry without producing a
            // reader event. An already armed retry also services that work.
            if self.retry_at.is_none()
                && (resize && result.is_ok()
                    || self
                        .sessions
                        .activity_mut()
                        .any(|(bridge, _)| bridge.needs_write_retry()))
            {
                self.retry_at = Some(Instant::now());
            }
            result
        };
        Reply {
            protocol: VERSION,
            request: request.request,
            result,
        }
    }

    fn dispatch(&mut self, command: Command) -> Result<HostReply, String> {
        let (id, result) = match command {
            Command::Identify => return HostReply::json(&self.identity),
            Command::Open { rows, cols } => {
                check_grid(rows, cols)?;
                let wake = Arc::clone(&self.wake);
                return self.sessions.admit(|| {
                    let mut bridge = LiveBridge::default();
                    let epoch = bridge.open_remote(rows, cols)?;
                    bridge.notify_on_output(move || wake.notify())?;
                    let owner = bridge.attachment(epoch)?;
                    let result = HostReply::json(&owner)?;
                    Ok((owner.session, bridge, result))
                });
            }
            Command::Close { owner } => {
                let bridge = self.bridge(&owner.session)?;
                let epoch = bridge.validate_attachment(&owner)?;
                let result = HostReply::json(bridge.close(epoch)?)?;
                self.sessions.remove(&owner.session);
                return Ok(result);
            }
            Command::AttachReadOnly { session, epoch } => {
                let bridge = self.bridge(&session)?;
                bridge.attachment(epoch)?;
                (session, HostReply::Attached(bridge.resume(None)?))
            }
            Command::AttachReadOnlyPacked { session, epoch } => {
                let bridge = self.bridge(&session)?;
                bridge.attachment(epoch)?;
                let view = bridge
                    .resume_ssh_packed(None)?
                    .ok_or("terminal session unavailable")?;
                (
                    session,
                    HostReply::PackedResume(PackedResume {
                        owner: view.owner,
                        frame: view.frame,
                        input_allowed: None,
                        keyboard_events_enabled: None,
                    }),
                )
            }
            Command::Recover { owner } => {
                let bridge = self.bridge(&owner.session)?;
                let resume = bridge
                    .resume(Some(&owner))?
                    .ok_or("terminal session unavailable")?;
                let input_allowed = bridge.recovered_input_allowed(&resume.owner);
                (
                    owner.session,
                    HostReply::Recovered {
                        view: resume,
                        input_allowed,
                        keyboard_events_enabled: bridge.remote_keyboard_events_enabled(),
                    },
                )
            }
            Command::RecoverPacked { owner } => {
                let bridge = self.bridge(&owner.session)?;
                let view = bridge
                    .resume_ssh_packed(Some(&owner))?
                    .ok_or("terminal session unavailable")?;
                let input_allowed = bridge.recovered_input_allowed(&view.owner);
                (
                    owner.session,
                    HostReply::PackedResume(PackedResume {
                        owner: view.owner,
                        frame: view.frame,
                        input_allowed: Some(input_allowed),
                        keyboard_events_enabled: Some(bridge.remote_keyboard_events_enabled()),
                    }),
                )
            }
            Command::Detach { owner } => {
                self.bridge(&owner.session)?.detach(&owner)?;
                (owner.session, HostReply::Json(Value::Null))
            }
            Command::Poll { owner } => self.attached(owner, false, |bridge, epoch| {
                bridge.poll_update(epoch).map(HostReply::Update)
            })?,
            Command::PollWithStatus {
                owner,
                ack_revision,
            } => self.attached(owner, false, |bridge, epoch| {
                if let Some(revision) = ack_revision {
                    bridge.acknowledge(epoch, revision)?;
                }
                let update = bridge.poll_update(epoch)?;
                let status = terminal_status(bridge, epoch)?;
                Ok(HostReply::PollWithStatus(PollReply { update, status }))
            })?,
            Command::PollPacked {
                owner,
                ack_revision,
                snapshot,
                frame_version,
            } => self.attached(owner, false, |bridge, epoch| {
                let version = frame_version.unwrap_or(RENDITION_VERSION);
                if !matches!(version, RENDITION_VERSION | SSH_DENSE_FRAME_VERSION) {
                    return Err("unsupported packed terminal frame version".into());
                }
                if let Some(revision) = ack_revision {
                    bridge.acknowledge_publication(epoch, revision.get())?;
                }
                let update = bridge
                    .poll_ssh_frame(epoch, snapshot, version)?
                    .map(|(_, frame)| frame);
                let status = terminal_status(bridge, epoch)?;
                Ok(HostReply::PollPacked(PollReply { update, status }))
            })?,
            Command::Ack { owner, revision } => self.attached(owner, false, |bridge, epoch| {
                HostReply::json(bridge.acknowledge(epoch, revision)?)
            })?,
            Command::AckPacked { owner, revision } => {
                self.attached(owner, false, |bridge, epoch| {
                    HostReply::json(bridge.acknowledge_publication(epoch, revision.get())?)
                })?
            }
            Command::Status { owner } => self.attached(owner, false, |bridge, epoch| {
                terminal_status(bridge, epoch).map(HostReply::Json)
            })?,
            Command::History { owner, offset } => {
                self.attached(owner, false, |bridge, epoch| {
                    bridge.history(epoch, offset).map(HostReply::History)
                })?
            }
            Command::Write { owner, bytes } => {
                check_input(bytes.len())?;
                let result = self
                    .bridge(&owner.session)?
                    .write_owned_raw(owner.clone(), bytes)?;
                (owner.session, HostReply::json(result)?)
            }
            Command::Key {
                owner,
                name,
                shift,
                alt,
                ctrl,
                keypad,
            } => {
                check_input(name.len())?;
                self.attached(owner, true, |bridge, epoch| {
                    HostReply::json(bridge.key_with_keypad(
                        epoch,
                        &name,
                        shift,
                        alt,
                        ctrl,
                        keypad.as_ref(),
                    )?)
                })?
            }
            Command::EnableKeyboardEvents { owner } => {
                self.attached(owner, true, |bridge, epoch| {
                    bridge.enable_remote_keyboard_events(epoch)?;
                    Ok(HostReply::Json(Value::Bool(true)))
                })?
            }
            Command::KeyEvent {
                owner,
                event,
                layout,
            } => self.attached(owner, true, |bridge, epoch| {
                let native = layout.map(KeyboardLayout::into_native).transpose()?;
                HostReply::json(bridge.key_event(epoch, &event, native)?)
            })?,
            Command::Text { owner, text } => {
                check_input(text.len())?;
                self.attached(owner, true, |bridge, epoch| {
                    HostReply::json(bridge.text(epoch, &text)?)
                })?
            }
            Command::Paste { owner, text } => {
                check_input(text.len())?;
                self.attached(owner, true, |bridge, epoch| {
                    HostReply::json(bridge.paste(epoch, &text)?)
                })?
            }
            Command::Mouse { owner, report } => self.attached(owner, true, |bridge, epoch| {
                HostReply::json(bridge.mouse(epoch, report)?)
            })?,
            Command::Focus { owner, focused } => self.attached(owner, true, |bridge, epoch| {
                HostReply::json(bridge.focus(epoch, focused)?)
            })?,
            Command::Resize { owner, rows, cols } => {
                check_grid(rows, cols)?;
                self.attached(owner, true, |bridge, epoch| {
                    HostReply::json(bridge.resize(epoch, rows, cols)?)
                })?
            }
        };
        self.sessions.touch(&id, Instant::now());
        Ok(result)
    }

    fn bridge(&mut self, id: &str) -> Result<&mut LiveBridge, String> {
        self.sessions
            .get_mut(id)
            .ok_or("terminal session unavailable".into())
    }

    fn attached(
        &mut self,
        owner: Attachment,
        input: bool,
        operation: impl FnOnce(&mut LiveBridge, u32) -> Result<HostReply, String>,
    ) -> Result<(String, HostReply), String> {
        let bridge = self.bridge(&owner.session)?;
        let epoch = if input {
            bridge.validate_logical_input(&owner)?
        } else {
            bridge.validate_attachment(&owner)?
        };
        Ok((owner.session, operation(bridge, epoch)?))
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(crate) fn read_record<T: DeserializeOwned>(
    reader: &mut impl BufRead,
    limit: usize,
) -> io::Result<Option<T>> {
    read_record_bytes(reader, limit)?
        .map(|data| decode_record(&data))
        .transpose()
}

fn read_record_bytes(reader: &mut impl BufRead, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut data = Vec::new();
    io::Read::take(reader, (limit + 1) as u64).read_until(b'\n', &mut data)?;
    if data.is_empty() {
        return Ok(None);
    }
    if data.len() > limit || data.last() != Some(&b'\n') {
        return Err(invalid("oversized or incomplete terminal transport record"));
    }
    Ok(Some(data))
}

fn decode_record<T: DeserializeOwned>(data: &[u8]) -> io::Result<T> {
    serde_json::from_slice(data).map_err(|_| invalid("malformed terminal transport JSON"))
}

pub(crate) fn write_record(
    writer: &mut impl Write,
    value: &impl Serialize,
    limit: usize,
) -> io::Result<()> {
    let data = serde_json::to_vec(value)
        .map_err(|_| invalid("terminal transport serialization failed"))?;
    if data.len() >= limit {
        return Err(invalid("terminal transport record exceeds byte limit"));
    }
    writer.write_all(&data)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn write_owner_reply(
    writer: &mut impl Write,
    reply: &Reply<HostReply>,
    limit: usize,
) -> io::Result<()> {
    let (field, frame, metadata) = match &reply.result {
        Ok(HostReply::PollPacked(poll)) => (
            "update",
            poll.update.as_deref().unwrap_or("null"),
            serde_json::json!({ "status": &poll.status }),
        ),
        Ok(HostReply::PackedResume(resume)) => (
            "frame",
            resume.frame.as_str(),
            serde_json::json!({
                "owner": &resume.owner,
                "input_allowed": resume.input_allowed,
                "keyboard_events_enabled": resume.keyboard_events_enabled,
            }),
        ),
        _ => return write_record(writer, reply, limit),
    };
    // The Channel encoder supplies bounded JSON. Write its bytes directly
    // into the reply. The final brace of the metadata object closes Ok.
    let header = format!(
        r#"{{"protocol":{},"request":{},"result":{{"Ok":{{"{field}":"#,
        reply.protocol, reply.request,
    );
    let metadata = serde_json::to_vec(&metadata)
        .map_err(|_| invalid("terminal transport serialization failed"))?;
    let parts = [
        header.as_bytes(),
        frame.as_bytes(),
        b",".as_slice(),
        &metadata[1..],
        b"}}\n",
    ];
    if parts.iter().map(|part| part.len()).sum::<usize>() > limit {
        return Err(invalid("terminal transport record exceeds byte limit"));
    }
    for part in parts {
        writer.write_all(part)?;
    }
    writer.flush()
}

fn current_uid() -> u32 {
    // SAFETY: geteuid has no arguments and returns the current process credential.
    unsafe { libc::geteuid() }
}

pub fn default_socket() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", current_uid())))
        .join("aether/terminal.sock")
}

fn private_parent(socket: &Path, create: bool) -> io::Result<()> {
    let parent = socket
        .parent()
        .filter(|_| socket.is_absolute() && socket.file_name().is_some())
        .ok_or_else(|| invalid("terminal socket requires an absolute path"))?;
    if create {
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err(invalid(
            "terminal socket parent must be a private directory owned by this account",
        ));
    }
    Ok(())
}

fn check_socket(socket: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(socket)?;
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid() {
        return Err(invalid("terminal endpoint is not an owned Unix socket"));
    }
    Ok(metadata)
}

struct OwnerLock(File);

impl Drop for OwnerLock {
    fn drop(&mut self) {
        // PTY children can inherit the open file description briefly between
        // fork and exec. Closing our descriptor alone leaves their lock live.
        let _ = File::unlock(&self.0);
    }
}

struct BoundSocket {
    path: PathBuf,
    identity: (u64, u64),
    _lock: OwnerLock,
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        if check_socket(&self.path).is_ok_and(|meta| (meta.dev(), meta.ino()) == self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn bind(socket: &Path) -> io::Result<(UnixListener, BoundSocket)> {
    private_parent(socket, true)?;
    let mut lock_path = socket.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err(invalid(
            "terminal owner lock must be a private regular file",
        ));
    }
    lock.try_lock()?;
    let lock = OwnerLock(lock);
    match check_socket(socket) {
        Ok(_) => match UnixStream::connect(socket) {
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                fs::remove_file(socket)?
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "terminal owner is already listening",
                ))
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(socket)?;
    let metadata = check_socket(socket)?;
    let socket_file = BoundSocket {
        path: socket.into(),
        identity: (metadata.dev(), metadata.ino()),
        _lock: lock,
    };
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok((listener, socket_file))
}

fn limit_socket(socket: &UnixStream) -> io::Result<()> {
    socket.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    socket.set_write_timeout(Some(SOCKET_TIMEOUT))
}

/// Run the per-account owner independently of SSH transport lifetimes.
pub fn serve(socket: &Path) -> io::Result<()> {
    let (listener, _socket_file) = bind(socket)?;
    let mut host = Host::new().map_err(io::Error::other)?;
    let mut readiness = [
        libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: host.wake.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        let deadline = host.next_deadline();
        let timeout = deadline.map_or(-1, |at| {
            at.saturating_duration_since(Instant::now())
                .as_nanos()
                .div_ceil(1_000_000)
                .min(i32::MAX as u128) as i32
        });
        // SAFETY: readiness contains both live descriptors and is writable.
        if unsafe { libc::poll(readiness.as_mut_ptr(), readiness.len() as _, timeout) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if readiness
            .iter()
            .any(|fd| fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0)
        {
            return Err(io::Error::other(
                "terminal owner readiness descriptor failed",
            ));
        }
        let now = Instant::now();
        let notified = readiness[1].revents & libc::POLLIN != 0;
        if notified {
            // Clear before draining PTYs so notifications arriving during the
            // pass remain readable for the next iteration.
            host.wake.clear()?;
        }
        if notified || deadline.is_some_and(|at| now >= at) {
            host.pump(now);
            host.expire(now);
        }
        if readiness[0].revents & libc::POLLIN == 0 {
            continue;
        }
        match listener.accept() {
            Ok((mut peer, _)) => {
                if limit_socket(&peer).is_ok() {
                    if let Ok(Some(request)) = read_record(&mut BufReader::new(&peer), MAX_REQUEST)
                    {
                        let reply = host.reply(request);
                        let _ = write_owner_reply(&mut peer, &reply, MAX_REPLY);
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
}

/// Forward one bounded request at a time between authenticated SSH stdio and
/// its account's local owner. The owner never holds an idle SSH connection.
pub fn stdio(socket: &Path, mut reader: impl BufRead, mut writer: impl Write) -> io::Result<()> {
    private_parent(socket, false)?;
    while let Some(request) = read_record::<Request>(&mut reader, MAX_REQUEST)? {
        let metadata = check_socket(socket)?;
        if metadata.mode() & 0o077 != 0 {
            return Err(invalid(
                "terminal socket permissions must exclude other accounts",
            ));
        }
        let mut peer = UnixStream::connect(socket)?;
        limit_socket(&peer)?;
        write_record(&mut peer, &request, MAX_REQUEST)?;
        let mut owner = BufReader::new(peer);
        match request.command {
            Command::Status { .. } => {
                forward_status_reply(&mut owner, &mut writer, request.request, MAX_REPLY)?;
            }
            Command::Identify if request.request == IDENTIFY_CAPABILITIES_REQUEST => {
                forward_identify_reply(&mut owner, &mut writer, request.request, MAX_REPLY)?;
            }
            _ => forward_reply(&mut owner, &mut writer, request.request, MAX_REPLY)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Host {
        fn execute(&mut self, command: Command) -> Result<Value, String> {
            reply_value(self.dispatch(command)?)
        }

        fn wire_reply(&mut self, request: Request) -> Reply<Value> {
            let mut bytes = Vec::new();
            write_owner_reply(&mut bytes, &self.reply(request), MAX_REPLY).unwrap();
            read_record(&mut bytes.as_slice(), MAX_REPLY)
                .unwrap()
                .unwrap()
        }
    }

    fn run_with_private_pty_capacity() -> bool {
        let thread = std::thread::current();
        let test_name = thread.name().expect("named Rust test thread");
        const CHILD_TEST: &str = "AETHER_PTY_CAPACITY_TEST";
        if std::env::var(CHILD_TEST).as_deref() == Ok(test_name) {
            return false;
        }
        // Each runtime owns a process-wide deferred-reaper budget. A full host
        // must exercise its own limit without competing with unrelated PTYs.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD_TEST, test_name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated capacity test failed\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        true
    }

    #[test]
    fn releasing_owner_unlocks_even_if_a_fork_inherited_the_descriptor() {
        let file = std::env::temp_dir().join(format!(
            "aether-owner-lock-{}",
            crate::semantic::session_id().unwrap(),
        ));
        let owned = File::create_new(&file).unwrap();
        let inherited = owned.try_clone().unwrap();
        let contender = File::options().read(true).write(true).open(&file).unwrap();
        fs::remove_file(&file).unwrap();

        owned.try_lock().unwrap();
        let owner = OwnerLock(owned);
        assert!(contender.try_lock().is_err());
        drop(owner);
        contender.try_lock().unwrap();
        drop(inherited);
    }

    #[test]
    fn malformed_and_oversized_records_do_not_reach_the_owner() {
        let request =
            br#"{"protocol":1,"request":1,"command":{"kind":"open","rows":16,"cols":80}}"#;
        assert!(read_record::<Request>(&mut &request[..], 128).is_err());
        let mut line = request.to_vec();
        line.push(b'\n');
        assert!(read_record::<Request>(&mut &line[..], 32).is_err());
        assert!(read_record::<Request>(&mut &line[..], 128)
            .unwrap()
            .is_some());
        let wrong_schema = b"{\"protocol\":1,\"request\":1,\"command\":{\"kind\":\"open\",\"rows\":16,\"cols\":80,\"extra\":0}}\n";
        assert!(read_record::<Request>(&mut &wrong_schema[..], 128).is_err());
        let mut host = Host::new().unwrap();
        let reply = host.reply(Request {
            protocol: VERSION + 1,
            request: 1,
            host: None,
            command: Command::Open { rows: 16, cols: 80 },
        });
        assert!(reply.result.is_err());
        assert!(host.sessions.is_empty());
    }

    #[test]
    fn expected_host_is_checked_before_allocating_or_looking_up_a_pty() {
        let mut first = Host::new().unwrap();
        let mut replacement = Host::new().unwrap();
        let request = |host, command| Request {
            protocol: VERSION,
            request: 1,
            host,
            command,
        };
        let identity = first
            .wire_reply(request(None, Command::Identify))
            .result
            .unwrap();
        assert!(valid_identity(identity.as_str().unwrap()));
        assert_ne!(
            replacement
                .wire_reply(request(None, Command::Identify))
                .result
                .unwrap(),
            identity,
        );
        let identity = identity.as_str().unwrap().to_owned();
        for command in [
            Command::Open { rows: 16, cols: 80 },
            Command::AttachReadOnly {
                session: "f".repeat(32),
                epoch: 1,
            },
        ] {
            assert_eq!(
                replacement
                    .wire_reply(request(Some(identity.clone()), command))
                    .result,
                Err(HOST_CHANGED.into()),
            );
        }
        assert!(replacement.sessions.is_empty());
        assert_eq!(
            first
                .wire_reply(request(Some(identity.clone()), Command::Identify))
                .result,
            Ok(Value::String(identity)),
        );
    }

    #[test]
    fn remote_alternate_scroll_uses_existing_authenticated_mouse_transport() {
        use std::{ffi::OsString, path::Path};
        let argv = [
            "/usr/bin/python3",
            "-c",
            r#"
import os,tty
tty.setraw(0)
os.write(1,b'\x1b[?1049h\x1b[?1007hREADY')
got=b''
while len(got)<3:got+=os.read(0,3-len(got))
assert got==b'\x1b[A',got
os.write(1,b'DONE')
os.read(0,1)
"#,
        ]
        .map(OsString::from);
        let mut bridge = LiveBridge::default();
        let epoch = bridge
            .open_prepared_editor(4, 40, &argv, Path::new("/"), &[])
            .unwrap();
        let owner = bridge.attachment(epoch).unwrap();
        let mut host = Host::new().unwrap();
        host.sessions
            .admit(|| Ok((owner.session.clone(), bridge, Value::Null)))
            .unwrap();
        let wait = |host: &mut Host, marker: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                host.pump(Instant::now());
                let bridge = host.bridge(&owner.session).unwrap();
                let frame = bridge.recover_frame(epoch).unwrap();
                let text: String = frame
                    .rows
                    .iter()
                    .flatten()
                    .map(|c| c.text.as_str())
                    .collect();
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if text.contains(marker) {
                    break;
                }
                assert!(Instant::now() < deadline, "missing {marker}");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        wait(&mut host, "READY");
        let status = host
            .execute(Command::Status {
                owner: owner.clone(),
            })
            .unwrap();
        assert_eq!(status["mouse_reporting"], true);
        assert!(status.get("mouse").is_none());
        let view = status["alternate_scroll"].clone();
        assert_eq!(view.as_object().unwrap().len(), 3);
        let report = |kind: &str, button: Option<&str>| {
            serde_json::from_value::<crate::live::MouseReport>(serde_json::json!({"layout":view["layout"],"revision":view["revision"],"generation":view["generation"],"row":0,"col":0,"kind":kind,"button":button,"alt":false,"ctrl":false})).unwrap()
        };
        let send = |host: &mut Host, report| {
            let request = Request {
                protocol: VERSION,
                request: 1,
                host: Some(host.identity.clone()),
                command: Command::Mouse {
                    owner: owner.clone(),
                    report,
                },
            };
            host.wire_reply(request).result
        };
        assert_eq!(
            send(&mut host, report("press", Some("left"))).unwrap(),
            false
        );
        assert_eq!(send(&mut host, report("wheel_up", None)).unwrap(), true);
        wait(&mut host, "DONE");
        host.execute(Command::Detach {
            owner: owner.clone(),
        })
        .unwrap();
        assert!(send(&mut host, report("wheel_down", None)).is_err());
        let resumed = host
            .execute(Command::AttachReadOnly {
                session: owner.session.clone(),
                epoch,
            })
            .unwrap();
        let reader: Attachment = serde_json::from_value(resumed["owner"].clone()).unwrap();
        let status = host
            .execute(Command::Status {
                owner: reader.clone(),
            })
            .unwrap();
        assert!(status.get("alternate_scroll").is_none());
        assert!(host
            .execute(Command::Mouse {
                owner: reader.clone(),
                report: report("wheel_up", None)
            })
            .is_err());
        host.execute(Command::Close { owner: reader }).unwrap();
    }

    #[test]
    fn remote_mouse_reports_require_identity_lease_and_rendered_surface() {
        use std::{ffi::OsString, path::Path};
        let argv = [
            "/usr/bin/python3",
            "-c",
            r#"
import os,tty
tty.setraw(0)
def expect(want):
 got=b''
 while len(got)<len(want): got+=os.read(0,len(want)-len(got))
 assert got==want,got
os.write(1,b'\x1b[?1002;1006hREADY')
expect(b'\x1b[<0;3;2M')
os.write(1,b'PAINT')
expect(b'\x1b[<0;3;2m')
os.write(1,b'\x1b[?1006lMODE')
expect(b'\x1b[M\x20\x23\x22\x1b[M\x23\x23\x22')
os.write(1,b'DONE')
expect(b'.')
"#,
        ]
        .map(OsString::from);
        let mut bridge = LiveBridge::default();
        let epoch = bridge
            .open_prepared_editor(4, 80, &argv, Path::new("/"), &[])
            .unwrap();
        let owner = bridge.attachment(epoch).unwrap();
        let mut host = Host::new().unwrap();
        host.sessions
            .admit(|| Ok((owner.session.clone(), bridge, Value::Null)))
            .unwrap();
        let wait = |host: &mut Host, marker: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                host.pump(Instant::now());
                let bridge = host.bridge(&owner.session).unwrap();
                let frame = bridge.recover_frame(epoch).unwrap();
                let text: String = frame
                    .rows
                    .iter()
                    .flatten()
                    .map(|c| c.text.as_str())
                    .collect();
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if text.contains(marker) {
                    break;
                }
                assert!(Instant::now() < deadline, "missing {marker}");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let status = |host: &mut Host| {
            host.execute(Command::Status {
                owner: owner.clone(),
            })
            .unwrap()
        };
        let report = |view: &Value, kind: &str| serde_json::json!({"layout":view["layout"],"revision":view["revision"],"generation":view["generation"],"row":1,"col":2,"kind":kind,"button":"left","alt":false,"ctrl":false});
        let mouse = |host: &mut Host, view: &Value, kind: &str| {
            let request = Request {
                protocol: VERSION,
                request: 1,
                host: Some(host.identity.clone()),
                command: Command::Mouse {
                    owner: owner.clone(),
                    report: serde_json::from_value(report(view, kind)).unwrap(),
                },
            };
            host.wire_reply(request).result
        };
        wait(&mut host, "READY");
        let info = status(&mut host);
        assert_eq!(info["mouse_reporting"], true);
        let view = info["mouse"].clone();
        assert_eq!(view["tracking"], "drag");
        assert_eq!(
            host.wire_reply(Request {
                protocol: VERSION,
                request: 1,
                host: None,
                command: Command::Mouse {
                    owner: owner.clone(),
                    report: serde_json::from_value(report(&view, "press")).unwrap()
                }
            })
            .result,
            Err(HOST_CHANGED.into())
        );
        let mut stale = view.clone();
        stale["revision"] = Value::String("999999".into());
        assert_eq!(mouse(&mut host, &stale, "press").unwrap(), false);
        let mut wrong = view.clone();
        wrong["generation"] = Value::from(view["generation"].as_u64().unwrap() + 1);
        assert_eq!(mouse(&mut host, &wrong, "press").unwrap(), false);
        assert_eq!(mouse(&mut host, &view, "press").unwrap(), true);
        wait(&mut host, "PAINT");
        assert_eq!(
            mouse(&mut host, &view, "release").unwrap(),
            true,
            "held release survives visible repaint and old ledger eviction"
        );
        wait(&mut host, "MODE");
        assert_eq!(
            mouse(&mut host, &view, "press").unwrap(),
            false,
            "old encoding generation retired"
        );
        let fresh = status(&mut host)["mouse"].clone();
        assert_eq!(mouse(&mut host, &fresh, "press").unwrap(), true);
        assert_eq!(mouse(&mut host, &fresh, "release").unwrap(), true);
        wait(&mut host, "DONE");
        host.execute(Command::Detach {
            owner: owner.clone(),
        })
        .unwrap();
        assert!(mouse(&mut host, &fresh, "press").is_err());
        let resumed = host
            .execute(Command::AttachReadOnly {
                session: owner.session.clone(),
                epoch,
            })
            .unwrap();
        let reader: Attachment = serde_json::from_value(resumed["owner"].clone()).unwrap();
        assert!(host
            .execute(Command::Mouse {
                owner: reader.clone(),
                report: serde_json::from_value(report(&fresh, "press")).unwrap()
            })
            .is_err());
        host.execute(Command::Close { owner: reader }).unwrap();
    }

    #[test]
    fn quiet_owner_arms_io_for_queued_input_only_when_needed() {
        let mut host = Host::new().unwrap();
        let opened = host.execute(Command::Open { rows: 8, cols: 80 }).unwrap();
        let owner: Attachment = serde_json::from_value(opened).unwrap();
        let status = host.reply(Request {
            protocol: VERSION,
            request: 1,
            host: None,
            command: Command::Status {
                owner: owner.clone(),
            },
        });
        assert!(status.result.is_ok());
        assert!(host.retry_at.is_none());

        let input = host.reply(Request {
            protocol: VERSION,
            request: 2,
            host: None,
            command: Command::Text {
                owner: owner.clone(),
                text: "q".into(),
            },
        });
        assert!(input.result.is_ok());
        assert!(host.bridge(&owner.session).unwrap().needs_write_retry());
        assert_eq!(host.next_deadline(), Some(host.retry_at.unwrap()));
        host.execute(Command::Close { owner }).unwrap();
    }

    #[test]
    fn identity_capabilities_preserve_the_ordinary_identity_reply() {
        let mut host = Host::new().unwrap();
        let identify = |host: &mut Host, request| {
            host.wire_reply(Request {
                protocol: VERSION,
                request,
                host: None,
                command: Command::Identify,
            })
            .result
            .unwrap()
        };
        assert_eq!(identify(&mut host, 1), host.identity);
        let reply = identify(&mut host, IDENTIFY_CAPABILITIES_REQUEST);
        assert_eq!(reply["instance"], host.identity);
        assert_eq!(reply["packed_snapshot_version"], SSH_DENSE_FRAME_VERSION);
        assert!(reply.get("packed_snapshot_relay_version").is_none());
    }

    #[test]
    fn packed_resume_retries_the_same_snapshot_and_rotates_the_input_lease() {
        let mut host = Host::new().unwrap();
        let opened = host.execute(Command::Open { rows: 8, cols: 80 }).unwrap();
        let owner: Attachment = serde_json::from_value(opened).unwrap();
        let call = |host: &mut Host, command| {
            host.wire_reply(Request {
                protocol: VERSION,
                request: 1,
                host: Some(host.identity.clone()),
                command,
            })
            .result
        };
        let first = call(
            &mut host,
            Command::RecoverPacked {
                owner: owner.clone(),
            },
        )
        .unwrap();
        assert_eq!(first["frame"]["kind"], "snapshot");
        assert_eq!(first["frame"]["stamp"]["version"], SSH_DENSE_FRAME_VERSION);
        assert_eq!(first["input_allowed"], true);
        let next: Attachment = serde_json::from_value(first["owner"].clone()).unwrap();
        assert_eq!(next.generation, owner.generation + 1);
        assert_eq!(
            call(
                &mut host,
                Command::RecoverPacked {
                    owner: owner.clone()
                }
            )
            .unwrap(),
            first
        );
        let revision: Counter =
            serde_json::from_value(first["frame"]["stamp"]["revision"].clone()).unwrap();
        assert!(call(&mut host, Command::AckPacked { owner, revision }).is_err());
        assert!(call(
            &mut host,
            Command::AckPacked {
                owner: next.clone(),
                revision: Counter::new(u64::MAX).unwrap(),
            }
        )
        .is_err());
        assert_eq!(
            call(
                &mut host,
                Command::AckPacked {
                    owner: next.clone(),
                    revision
                }
            )
            .unwrap(),
            Value::Null
        );

        let resumed = call(
            &mut host,
            Command::AttachReadOnlyPacked {
                session: next.session.clone(),
                epoch: next.epoch,
            },
        )
        .unwrap();
        let reader: Attachment = serde_json::from_value(resumed["owner"].clone()).unwrap();
        assert_eq!(reader.generation, next.generation + 1);
        assert_eq!(
            resumed["frame"]["stamp"]["version"],
            SSH_DENSE_FRAME_VERSION
        );
        assert_eq!(
            call(
                &mut host,
                Command::Text {
                    owner: reader.clone(),
                    text: "x".into()
                }
            )
            .unwrap_err(),
            "reconnected terminal view is read-only"
        );
        host.execute(Command::Close { owner: reader }).unwrap();
    }

    #[test]
    fn packed_owner_reply_writes_its_frame_as_bounded_json() {
        let frame = r#"{ "kind":"snapshot", "rows":[] }"#;
        let reply = Reply {
            protocol: VERSION,
            request: 7,
            result: Ok(HostReply::PollPacked(PollReply {
                update: Some(frame.into()),
                status: serde_json::json!({"description":"Snow \"雪\""}),
            })),
        };
        let mut record = Vec::new();
        write_owner_reply(&mut record, &reply, 1024).unwrap();
        let wire = std::str::from_utf8(&record).unwrap();
        assert!(wire.contains(frame));
        let received: Reply = decode_record(&record).unwrap();
        let result = received.result.unwrap();
        assert_eq!(result["update"]["kind"], "snapshot");
        assert_eq!(result["status"]["description"], "Snow \"雪\"");
        let mut oversized = Vec::new();
        assert!(write_owner_reply(&mut oversized, &reply, record.len() - 1).is_err());
        assert!(oversized.is_empty());
    }

    #[test]
    fn packed_poll_keeps_the_publication_receipt_and_stops_at_a_retired_attachment() {
        let mut host = Host::new().unwrap();
        let owner: Attachment =
            serde_json::from_value(host.execute(Command::Open { rows: 8, cols: 80 }).unwrap())
                .unwrap();
        let poll = |host: &mut Host, bound: bool, snapshot, ack_revision| {
            host.wire_reply(Request {
                protocol: VERSION,
                request: 1,
                host: bound.then(|| host.identity.clone()),
                command: Command::PollPacked {
                    owner: owner.clone(),
                    ack_revision,
                    snapshot,
                    frame_version: None,
                },
            })
            .result
        };
        assert_eq!(
            poll(&mut host, false, true, None).unwrap_err(),
            HOST_CHANGED
        );
        let initial = host
            .execute(Command::Poll {
                owner: owner.clone(),
            })
            .unwrap();
        let revision = serde_json::from_value(initial["frame"]["revision"].clone()).unwrap();
        host.execute(Command::Ack {
            owner: owner.clone(),
            revision,
        })
        .unwrap();

        let first = poll(&mut host, true, true, None).unwrap();
        let frame = &first["update"];
        assert_eq!(frame["kind"], "snapshot");
        assert_eq!(frame["stamp"]["version"], RENDITION_VERSION);
        assert!(frame["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row.get("runs").is_some()));
        let revision: Counter = serde_json::from_value(frame["stamp"]["revision"].clone()).unwrap();
        let unsupported = host.wire_reply(Request {
            protocol: VERSION,
            request: 1,
            host: Some(host.identity.clone()),
            command: Command::PollPacked {
                owner: owner.clone(),
                ack_revision: Some(revision),
                snapshot: false,
                frame_version: Some(SSH_DENSE_FRAME_VERSION + 1),
            },
        });
        assert_eq!(
            unsupported.result.unwrap_err(),
            "unsupported packed terminal frame version"
        );
        assert_eq!(
            poll(&mut host, true, false, None).unwrap()["update"],
            Value::Null
        );
        assert_eq!(
            poll(&mut host, true, false, Counter::new(u64::MAX)).unwrap_err(),
            "wrong frame ACK"
        );

        let command = b"printf '\\033[4;1HPACKED_POLL'\n";
        assert_eq!(
            host.execute(Command::Write {
                owner: owner.clone(),
                bytes: command.to_vec()
            })
            .unwrap(),
            command.len()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut receipt = Some(revision);
        loop {
            let reply = poll(&mut host, true, false, receipt.take()).unwrap();
            if !reply["update"].is_null() {
                let update = &reply["update"];
                assert_eq!(update["kind"], "delta");
                assert_eq!(update["base_revision"], frame["stamp"]["revision"]);
                break;
            }
            assert!(Instant::now() < deadline, "packed delta did not arrive");
            std::thread::sleep(Duration::from_millis(2));
        }

        let recovered = host
            .execute(Command::Recover {
                owner: owner.clone(),
            })
            .unwrap();
        assert_eq!(
            poll(&mut host, true, true, None).unwrap_err(),
            "stale terminal attachment"
        );
        let current = serde_json::from_value(recovered["owner"].clone()).unwrap();
        host.execute(Command::Close { owner: current }).unwrap();
    }

    #[test]
    fn batched_poll_checks_the_host_and_applies_only_the_current_frame_receipt() {
        let mut host = Host::new().unwrap();
        let opened = host.execute(Command::Open { rows: 8, cols: 80 }).unwrap();
        let owner: Attachment = serde_json::from_value(opened).unwrap();
        let poll = |host: &mut Host, bound: bool, owner: &Attachment, ack_revision| {
            host.wire_reply(Request {
                protocol: VERSION,
                request: 1,
                host: bound.then(|| host.identity.clone()),
                command: Command::PollWithStatus {
                    owner: owner.clone(),
                    ack_revision,
                },
            })
            .result
        };
        assert_eq!(
            poll(&mut host, false, &owner, None).unwrap_err(),
            HOST_CHANGED
        );
        let first = poll(&mut host, true, &owner, None).unwrap();
        assert_eq!(first["status"]["epoch"], owner.epoch);
        assert_eq!(first["status"]["poll_with_status"], true);
        assert_eq!(first["update"]["kind"], "snapshot");
        let revision: u32 =
            serde_json::from_value(first["update"]["frame"]["revision"].clone()).unwrap();
        assert_eq!(
            poll(&mut host, true, &owner, Some(0)).unwrap_err(),
            "wrong frame ACK"
        );
        host.execute(Command::Resize {
            owner: owner.clone(),
            rows: 9,
            cols: 81,
        })
        .unwrap();
        assert_eq!(
            poll(&mut host, true, &owner, None).unwrap()["update"],
            Value::Null
        );
        let next = poll(&mut host, true, &owner, Some(revision)).unwrap();
        assert_eq!(next["update"]["frame"]["cols"], 81);
        let next_revision: u32 =
            serde_json::from_value(next["update"]["frame"]["revision"].clone()).unwrap();
        assert!(next_revision > revision);

        let recovered = host
            .execute(Command::Recover {
                owner: owner.clone(),
            })
            .unwrap();
        assert_eq!(
            poll(&mut host, true, &owner, Some(next_revision)).unwrap_err(),
            "stale terminal attachment"
        );
        let reader = serde_json::from_value(recovered["owner"].clone()).unwrap();
        host.execute(Command::Close { owner: reader }).unwrap();
    }

    #[test]
    fn remote_keyboard_opt_in_requires_a_bound_writer() {
        let mut host = Host::new().unwrap();
        let opened = host.execute(Command::Open { rows: 8, cols: 80 }).unwrap();
        let owner = serde_json::from_value::<Attachment>(opened).unwrap();
        let status = host
            .execute(Command::Status {
                owner: owner.clone(),
            })
            .unwrap();
        assert_eq!(status["keyboard_events_supported"], true);
        let send = |host: &mut Host, bound: bool, command| {
            host.wire_reply(Request {
                protocol: VERSION,
                request: 1,
                host: bound.then(|| host.identity.clone()),
                command,
            })
            .result
        };
        let enable = |owner: &Attachment| Command::EnableKeyboardEvents {
            owner: owner.clone(),
        };
        let release = |owner: &Attachment| {
            serde_json::from_value::<Command>(serde_json::json!({
                "kind": "key_event", "owner": owner,
                "event": {
                    "phase": "release", "name": "F3", "code": "F3",
                    "shift": false, "alt": false, "ctrl": false, "super_key": false,
                    "caps_lock": false, "num_lock": false, "focus_generation": 0
                }
            }))
            .unwrap()
        };
        assert!(send(&mut host, true, release(&owner)).is_err());
        assert_eq!(
            send(&mut host, false, enable(&owner)).unwrap_err(),
            HOST_CHANGED
        );
        assert_eq!(send(&mut host, true, enable(&owner)).unwrap(), true);
        assert_eq!(
            send(&mut host, false, release(&owner)).unwrap_err(),
            HOST_CHANGED
        );
        assert_eq!(send(&mut host, true, release(&owner)).unwrap(), false);

        let recovered = host
            .execute(Command::Recover {
                owner: owner.clone(),
            })
            .unwrap();
        assert_eq!(recovered["keyboard_events_enabled"], true);
        assert!(send(&mut host, true, release(&owner)).is_err());
        let reader = host
            .execute(Command::AttachReadOnly {
                session: owner.session,
                epoch: owner.epoch,
            })
            .unwrap();
        let reader = serde_json::from_value::<Attachment>(reader["owner"].clone()).unwrap();
        assert!(send(&mut host, true, enable(&reader)).is_err());
        assert!(send(&mut host, true, release(&reader)).is_err());
        host.execute(Command::Close { owner: reader }).unwrap();
    }

    #[test]
    fn remote_focus_capability_uses_live_pty_modes_and_writer_leases() {
        use std::{ffi::OsString, path::Path};
        let argv = [
            "/usr/bin/python3",
            "-c",
            r#"
import os, tty
tty.setraw(0)
def expect(want):
    got=b''
    while len(got)<len(want): got+=os.read(0,len(want)-len(got))
    assert got==want,got
os.write(1,b'\x1b[?1004hREADY')
expect(b'\x1b[I\x1b[O')
os.write(1,b'\x1b[?1004lDISABLED')
expect(b'!')
os.write(1,b'\x1b[?1004hENABLED')
expect(b'\x1b[I')
os.write(1,b'FOCUSED')
expect(b'\x1b[O')
os.write(1,b'RETIRED')
"#,
        ]
        .map(OsString::from);
        let mut bridge = LiveBridge::default();
        let epoch = bridge
            .open_prepared_editor(4, 80, &argv, Path::new("/"), &[])
            .unwrap();
        let owner = bridge.attachment(epoch).unwrap();
        let mut host = Host::new().unwrap();
        host.sessions
            .admit(|| Ok((owner.session.clone(), bridge, Value::Null)))
            .unwrap();
        let wait = |host: &mut Host, marker: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                host.pump(Instant::now());
                let frame = host
                    .bridge(&owner.session)
                    .unwrap()
                    .recover_frame(epoch)
                    .unwrap();
                let text: String = frame
                    .rows
                    .iter()
                    .flatten()
                    .map(|c| c.text.as_str())
                    .collect();
                if text.contains(marker) {
                    break;
                }
                assert!(Instant::now() < deadline, "missing {marker}");
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        wait(&mut host, "READY");
        assert_eq!(
            host.wire_reply(Request {
                protocol: VERSION,
                request: 1,
                host: None,
                command: Command::Focus {
                    owner: owner.clone(),
                    focused: true
                }
            })
            .result,
            Err(HOST_CHANGED.into())
        );

        assert_eq!(
            host.execute(Command::Status {
                owner: owner.clone()
            })
            .unwrap()["focus_reporting"],
            true
        );
        assert_eq!(
            host.execute(Command::Focus {
                owner: owner.clone(),
                focused: true
            })
            .unwrap(),
            true
        );
        assert_eq!(
            host.execute(Command::Focus {
                owner: owner.clone(),
                focused: true
            })
            .unwrap(),
            false
        );
        assert_eq!(
            host.execute(Command::Focus {
                owner: owner.clone(),
                focused: false
            })
            .unwrap(),
            true
        );
        wait(&mut host, "DISABLED");
        assert_eq!(
            host.execute(Command::Focus {
                owner: owner.clone(),
                focused: true
            })
            .unwrap(),
            false
        );
        assert_eq!(
            host.execute(Command::Focus {
                owner: owner.clone(),
                focused: false
            })
            .unwrap(),
            false
        );
        host.execute(Command::Text {
            owner: owner.clone(),
            text: "!".into(),
        })
        .unwrap();
        wait(&mut host, "ENABLED");
        assert_eq!(
            host.execute(Command::Focus {
                owner: owner.clone(),
                focused: true
            })
            .unwrap(),
            true
        );
        wait(&mut host, "FOCUSED");
        host.execute(Command::Detach {
            owner: owner.clone(),
        })
        .unwrap();
        wait(&mut host, "RETIRED");
        assert!(host
            .execute(Command::Focus {
                owner: owner.clone(),
                focused: false
            })
            .is_err());
        let resumed = host
            .execute(Command::AttachReadOnly {
                session: owner.session.clone(),
                epoch,
            })
            .unwrap();
        let reader: Attachment = serde_json::from_value(resumed["owner"].clone()).unwrap();
        assert!(host
            .execute(Command::Focus {
                owner: reader.clone(),
                focused: true
            })
            .is_err());
        host.execute(Command::Close { owner: reader }).unwrap();
    }

    #[test]
    fn host_limits_live_ptys_and_expires_abandoned_sessions() {
        if run_with_private_pty_capacity() {
            return;
        }
        let mut host = Host::new().unwrap();
        assert!(host.execute(Command::Open { rows: 0, cols: 80 }).is_err());
        for _ in 0..MAX_SESSIONS {
            host.execute(Command::Open { rows: 16, cols: 80 }).unwrap();
        }
        assert_eq!(
            host.execute(Command::Open { rows: 16, cols: 80 })
                .unwrap_err(),
            "terminal host session limit reached"
        );
        host.expire(Instant::now() + INACTIVITY);
        assert!(host.sessions.is_empty());
    }

    #[test]
    fn stale_close_at_capacity_cannot_release_another_session_or_its_replacement() {
        if run_with_private_pty_capacity() {
            return;
        }
        fn open(host: &mut Host) -> Result<Attachment, String> {
            let result = host.execute(Command::Open { rows: 16, cols: 80 })?;
            Ok(serde_json::from_value(result).unwrap())
        }

        let mut host = Host::new().unwrap();
        let a = open(&mut host).unwrap();
        let b = open(&mut host).unwrap();
        assert_ne!(a.session, b.session);
        assert_eq!(a.epoch, b.epoch);
        assert_eq!(a.generation, b.generation);
        for _ in 2..MAX_SESSIONS {
            open(&mut host).unwrap();
        }
        let mut resumed = host.execute(Command::Recover { owner: a.clone() }).unwrap();
        assert_eq!(resumed["input_allowed"], true);
        let resumed: Attachment = serde_json::from_value(resumed["owner"].take()).unwrap();
        assert_eq!(resumed.session, a.session);
        assert_eq!(resumed.epoch, a.epoch);
        assert_eq!(resumed.generation, a.generation + 1);

        assert_eq!(
            host.execute(Command::Close { owner: a.clone() })
                .unwrap_err(),
            "stale terminal attachment"
        );
        assert_eq!(
            host.execute(Command::Write {
                owner: a.clone(),
                bytes: vec![b'x'],
            })
            .unwrap_err(),
            "stale terminal attachment"
        );
        assert_eq!(
            open(&mut host).unwrap_err(),
            "terminal host session limit reached"
        );
        host.execute(Command::Status { owner: b.clone() }).unwrap();

        host.execute(Command::Close {
            owner: resumed.clone(),
        })
        .unwrap();
        let replacement = open(&mut host).unwrap();
        assert_ne!(replacement.session, a.session);
        assert_ne!(replacement.session, b.session);
        assert_eq!(replacement.epoch, a.epoch);
        assert_eq!(replacement.generation, a.generation);
        for old in [a, resumed] {
            assert_eq!(
                host.execute(Command::Close { owner: old }).unwrap_err(),
                "terminal session unavailable"
            );
        }
        host.execute(Command::Status { owner: replacement })
            .unwrap();
        host.execute(Command::Status { owner: b }).unwrap();
        assert_eq!(
            open(&mut host).unwrap_err(),
            "terminal host session limit reached"
        );
    }

    #[test]
    fn unattended_exit_output_renews_retention_and_recovers_read_only() {
        let mut host = Host::new().unwrap();
        let opened = host.execute(Command::Open { rows: 8, cols: 80 }).unwrap();
        let owner: Attachment = serde_json::from_value(opened).unwrap();
        let bytes = b"printf '%s%s\\n' EXIT_ WITNESS; exit\n";
        assert_eq!(
            host.execute(Command::Write {
                owner: owner.clone(),
                bytes: bytes.to_vec(),
            })
            .unwrap(),
            bytes.len()
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let expires_without_output = Instant::now() + INACTIVITY;
        loop {
            host.pump(expires_without_output);
            if host
                .bridge(&owner.session)
                .unwrap()
                .status(owner.epoch)
                .unwrap()
                .drained
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        host.expire(expires_without_output);
        let result = host
            .execute(Command::Recover {
                owner: owner.clone(),
            })
            .unwrap();
        assert_eq!(result["input_allowed"], false);
        let recovered: crate::semantic::Resume = serde_json::from_value(result.clone()).unwrap();
        assert!(recovered.frame.rows.iter().any(|row| row
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("EXIT_WITNESS")));
        assert_eq!(host.execute(Command::Recover { owner }).unwrap(), result);
    }

    #[test]
    fn owner_rejects_foreign_paths_and_replaces_only_a_stale_owned_socket() {
        struct Directory(PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
            "aether-transport-{}",
            crate::semantic::session_id().unwrap(),
        )));
        let path = directory.0.join("terminal.sock");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory.0)
            .unwrap();
        fs::write(&path, b"unrelated").unwrap();
        assert!(bind(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"unrelated");
        fs::remove_file(&path).unwrap();
        let stale = UnixListener::bind(&path).unwrap();
        // Stop accepting before dropping the descriptor. A parallel PTY test
        // can briefly inherit this descriptor between fork and exec.
        assert_eq!(
            unsafe { libc::shutdown(stale.as_raw_fd(), libc::SHUT_RDWR) },
            0
        );
        drop(stale);
        let (listener, socket_file) = bind(&path).unwrap();
        assert_eq!(check_socket(&path).unwrap().mode() & 0o777, 0o600);
        assert!(bind(&path).is_err());
        drop(listener);
        drop(socket_file);
        assert!(!path.exists());
    }
}
