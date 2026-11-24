//! Private Linux PTY owner and versioned RPC records for OpenSSH stdio.
mod session_table;
pub mod ssh;

use crate::{live::LiveBridge, semantic::Attachment};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use session_table::SessionTable;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    os::fd::AsRawFd,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const VERSION: u8 = 1;
pub const HOST_CHANGED: &str = "terminal host instance changed";
const MAX_REQUEST: usize = 128 * 1024;
const MAX_REPLY: usize = 16 * 1024 * 1024;
const MAX_INPUT: usize = 16 * 1024;
const MAX_SESSIONS: usize = 4;
const INACTIVITY: Duration = Duration::from_secs(10 * 60);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);
const ACTIVE_IO_TICK: Duration = Duration::from_millis(25);
const IDLE_OWNER_TICK: Duration = Duration::from_millis(1000);

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
    Recover {
        owner: Attachment,
    },
    Detach {
        owner: Attachment,
    },
    Poll {
        owner: Attachment,
    },
    Ack {
        owner: Attachment,
        revision: u32,
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
pub struct Reply<T = Value> {
    pub protocol: u8,
    pub request: u32,
    pub result: Result<T, String>,
}

struct Host {
    identity: String,
    sessions: SessionTable<LiveBridge, MAX_SESSIONS>,
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
        })
    }

    fn expire(&mut self, now: Instant) {
        self.sessions.expire(now, INACTIVITY);
    }

    fn pump(&mut self, now: Instant) -> bool {
        let mut active = false;
        for (bridge, last_used) in self.sessions.activity_mut() {
            if let Ok((running, output_bytes)) = bridge.pump_unpublished_with_activity() {
                active |= running;
                if output_bytes != 0 {
                    *last_used = now;
                }
            }
        }
        active
    }

    fn reply(&mut self, request: Request) -> Reply {
        let result = if request.protocol != VERSION || request.request == 0 {
            Err("unsupported terminal transport version or request ID".into())
        } else if request
            .host
            .as_ref()
            .is_some_and(|host| host != &self.identity)
        {
            Err(HOST_CHANGED.into())
        } else {
            self.execute(request.command)
        };
        Reply {
            protocol: VERSION,
            request: request.request,
            result,
        }
    }

    fn execute(&mut self, command: Command) -> Result<Value, String> {
        let (id, result) = match command {
            Command::Identify => return reply_value(&self.identity),
            Command::Open { rows, cols } => {
                check_grid(rows, cols)?;
                return self.sessions.admit(|| {
                    let mut bridge = LiveBridge::default();
                    let epoch = bridge.open(rows, cols)?;
                    let owner = bridge.attachment(epoch)?;
                    let result = reply_value(&owner)?;
                    Ok((owner.session, bridge, result))
                });
            }
            Command::Close { owner } => {
                let bridge = self.bridge(&owner.session)?;
                let epoch = bridge.validate_attachment(&owner)?;
                let result = reply_value(bridge.close(epoch)?)?;
                self.sessions.remove(&owner.session);
                return Ok(result);
            }
            Command::AttachReadOnly { session, epoch } => {
                let bridge = self.bridge(&session)?;
                bridge.attachment(epoch)?;
                (session, reply_value(bridge.resume(None)?)?)
            }
            Command::Recover { owner } => {
                let bridge = self.bridge(&owner.session)?;
                let resume = bridge
                    .resume(Some(&owner))?
                    .ok_or("terminal session unavailable")?;
                let input_allowed = bridge.recovered_input_allowed(&resume.owner);
                let mut result = reply_value(resume)?;
                result["input_allowed"] = Value::Bool(input_allowed);
                (owner.session, result)
            }
            Command::Detach { owner } => {
                self.bridge(&owner.session)?.detach(&owner)?;
                (owner.session, Value::Null)
            }
            Command::Poll { owner } => self.attached(owner, false, |bridge, epoch| {
                reply_value(bridge.poll_update(epoch)?)
            })?,
            Command::Ack { owner, revision } => self.attached(owner, false, |bridge, epoch| {
                reply_value(bridge.acknowledge(epoch, revision)?)
            })?,
            Command::Status { owner } => self.attached(owner, false, |bridge, epoch| {
                reply_value(bridge.status(epoch)?)
            })?,
            Command::History { owner, offset } => {
                self.attached(owner, false, |bridge, epoch| {
                    reply_value(bridge.history(epoch, offset)?)
                })?
            }
            Command::Write { owner, bytes } => {
                check_input(bytes.len())?;
                let result = self
                    .bridge(&owner.session)?
                    .write_owned_raw(owner.clone(), bytes)?;
                (owner.session, reply_value(result)?)
            }
            Command::Key {
                owner,
                name,
                shift,
                alt,
                ctrl,
            } => {
                check_input(name.len())?;
                self.attached(owner, true, |bridge, epoch| {
                    reply_value(bridge.key(epoch, &name, shift, alt, ctrl)?)
                })?
            }
            Command::Text { owner, text } => {
                check_input(text.len())?;
                self.attached(owner, true, |bridge, epoch| {
                    reply_value(bridge.text(epoch, &text)?)
                })?
            }
            Command::Paste { owner, text } => {
                check_input(text.len())?;
                self.attached(owner, true, |bridge, epoch| {
                    reply_value(bridge.paste(epoch, &text)?)
                })?
            }
            Command::Resize { owner, rows, cols } => {
                check_grid(rows, cols)?;
                self.attached(owner, true, |bridge, epoch| {
                    reply_value(bridge.resize(epoch, rows, cols)?)
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
        operation: impl FnOnce(&mut LiveBridge, u32) -> Result<Value, String>,
    ) -> Result<(String, Value), String> {
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
    let mut data = Vec::new();
    io::Read::take(reader, (limit + 1) as u64).read_until(b'\n', &mut data)?;
    if data.is_empty() {
        return Ok(None);
    }
    if data.len() > limit || data.last() != Some(&b'\n') {
        return Err(invalid("oversized or incomplete terminal transport record"));
    }
    serde_json::from_slice(&data)
        .map(Some)
        .map_err(|_| invalid("malformed terminal transport JSON"))
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
    let mut readiness = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let mut next_tick = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_tick {
            let active = host.pump(now);
            host.expire(now);
            next_tick = now
                + if active {
                    ACTIVE_IO_TICK
                } else {
                    IDLE_OWNER_TICK
                };
        }
        match listener.accept() {
            Ok((mut peer, _)) => {
                if limit_socket(&peer).is_ok() {
                    if let Ok(Some(request)) = read_record(&mut BufReader::new(&peer), MAX_REQUEST)
                    {
                        let reply = host.reply(request);
                        let _ = write_record(&mut peer, &reply, MAX_REPLY);
                        next_tick = next_tick.min(Instant::now() + ACTIVE_IO_TICK);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // SAFETY: readiness is a valid writable pollfd for the owned listener.
                // Wake for PTY reads and expiry even with no relay traffic.
                let timeout = next_tick
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .max(1);
                if unsafe { libc::poll(&mut readiness, 1, timeout as i32) } < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
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
        let reply: Reply = read_record(&mut BufReader::new(peer), MAX_REPLY)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal owner closed during request",
            )
        })?;
        if reply.protocol != VERSION || reply.request != request.request {
            return Err(invalid("terminal owner reply does not match request"));
        }
        write_record(&mut writer, &reply, MAX_REPLY)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .reply(request(None, Command::Identify))
            .result
            .unwrap();
        assert!(valid_identity(identity.as_str().unwrap()));
        assert_ne!(
            replacement
                .reply(request(None, Command::Identify))
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
                    .reply(request(Some(identity.clone()), command))
                    .result,
                Err(HOST_CHANGED.into()),
            );
        }
        assert!(replacement.sessions.is_empty());
        assert_eq!(
            first
                .reply(request(Some(identity.clone()), Command::Identify))
                .result,
            Ok(Value::String(identity)),
        );
    }

    #[test]
    fn host_limits_live_ptys_and_expires_abandoned_sessions() {
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
        while host.pump(expires_without_output) {
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
