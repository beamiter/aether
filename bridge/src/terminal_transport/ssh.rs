//! One authenticated OpenSSH relay, with an absolute deadline per RPC.
use super::{
    invalid, read_record, reply_forward::read_validated_reply, valid_identity, write_record,
    Command, Reply, Request, HOST_CHANGED, IDENTIFY_CAPABILITIES_REQUEST, MAX_REPLY, MAX_REQUEST,
    VERSION,
};
use crate::SSH_DENSE_FRAME_VERSION;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::{
    io::{self, BufReader, Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command as ProcessCommand, Stdio},
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct Endpoint {
    destination: String,
    remote_command: String,
}

impl Endpoint {
    /// `destination` is an OpenSSH host alias or [user@]host. The remote
    /// service paths come from the trusted native application's settings.
    pub fn new(
        destination: &str,
        helper: Option<&Path>,
        socket: Option<&Path>,
    ) -> Result<Self, String> {
        Self::helper(destination, helper, "stdio", socket)
    }

    pub fn files(destination: &str, helper: Option<&Path>) -> Result<Self, String> {
        Self::helper(destination, helper, "files", None)
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    fn helper(
        destination: &str,
        helper: Option<&Path>,
        mode: &str,
        socket: Option<&Path>,
    ) -> Result<Self, String> {
        let destination = destination.trim();
        if destination.is_empty()
            || destination.len() > 255
            || !destination.bytes().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, b'.' | b'-' | b'_' | b'@' | b':' | b'[' | b']')
            })
            || destination.split('@').count() > 2
            || destination
                .split('@')
                .any(|part| part.is_empty() || part.starts_with('-'))
        {
            return Err("Enter an SSH host alias or user@host.".into());
        }
        let mut remote_command = match helper {
            Some(path) => format!("exec {} {mode}", quote_absolute_path(path)?),
            None => format!("exec aether-terminal-host {mode}"),
        };
        if let Some(socket) = socket {
            remote_command.push(' ');
            remote_command.push_str(&quote_absolute_path(socket)?);
        }
        Ok(Self {
            destination: destination.into(),
            remote_command,
        })
    }

    pub fn connect(&self) -> io::Result<Relay> {
        let (channel, peer) = UnixStream::pair()?;
        let stdin: OwnedFd = peer.try_clone()?.into();
        let stdout: OwnedFd = peer.into();
        let child = ProcessCommand::new("ssh")
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=1",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "PermitLocalCommand=no",
                "--",
            ])
            .arg(&self.destination)
            .arg(&self.remote_command)
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Relay {
            child,
            channel: BufReader::new(DeadlineStream {
                channel,
                deadline: Instant::now(),
            }),
            next_request: 1,
            host: None,
            packed_snapshots: false,
        })
    }
}

fn quote_absolute_path(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .filter(|value| {
            path.is_absolute() && value.len() <= 4096 && !value.chars().any(char::is_control)
        })
        .ok_or("SSH terminal service paths must be absolute UTF-8 paths.")?;
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

struct DeadlineStream {
    channel: UnixStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|time| !time.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "SSH terminal request timed out")
            })
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.channel.set_read_timeout(Some(self.remaining()?))?;
        self.channel.read(buf)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.channel.set_write_timeout(Some(self.remaining()?))?;
        self.channel.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.channel.flush()
    }
}

pub struct Relay {
    child: Child,
    channel: BufReader<DeadlineStream>,
    next_request: u32,
    host: Option<String>,
    packed_snapshots: bool,
}

impl Relay {
    /// Discover the owner before sending any saved session identity. Bind
    /// subsequent requests as stdio can reach a replacement Unix socket even
    /// on the same authenticated SSH connection.
    pub fn bind_host(&mut self, expected: Option<&str>) -> Result<String, String> {
        let reply: Value = self
            .exchange_with_id(
                IDENTIFY_CAPABILITIES_REQUEST,
                Command::Identify,
                MAX_REQUEST,
                MAX_REPLY,
                read_record,
            )
            .map_err(|_| "SSH terminal owner identity is unavailable.")??;
        let identity = reply
            .as_str()
            .or_else(|| reply.get("instance").and_then(Value::as_str))
            .filter(|identity| valid_identity(identity))
            .ok_or("Invalid SSH terminal owner identity.")?;
        if expected.is_some_and(|expected| expected != identity) {
            return Err(HOST_CHANGED.into());
        }
        self.host = Some(identity.to_owned());
        let version = Some(u64::from(SSH_DENSE_FRAME_VERSION));
        self.packed_snapshots = reply.get("packed_snapshot_version").and_then(Value::as_u64)
            == version
            && reply
                .get("packed_snapshot_relay_version")
                .and_then(Value::as_u64)
                == version;
        Ok(identity.to_owned())
    }

    pub fn supports_packed_snapshots(&self) -> bool {
        self.packed_snapshots
    }

    /// An I/O error ends the relay. The caller can recover the known session
    /// on a new relay without retransmitting the interrupted operation.
    pub fn call(&mut self, command: Command) -> io::Result<Result<Value, String>> {
        self.exchange(command, MAX_REQUEST, MAX_REPLY)
    }

    /// Receive a frame response with full JSON validation and a raw payload.
    pub fn call_frame<T: DeserializeOwned>(
        &mut self,
        command: Command,
    ) -> io::Result<Result<T, String>> {
        self.exchange_with(command, MAX_REQUEST, MAX_REPLY, read_validated_reply)
    }

    pub(crate) fn exchange<C: Serialize, T: DeserializeOwned>(
        &mut self,
        command: C,
        request_limit: usize,
        response_limit: usize,
    ) -> io::Result<Result<T, String>> {
        self.exchange_with(command, request_limit, response_limit, read_record)
    }

    fn exchange_with<C: Serialize, T>(
        &mut self,
        command: C,
        request_limit: usize,
        response_limit: usize,
        read_reply: impl FnOnce(&mut BufReader<DeadlineStream>, usize) -> io::Result<Option<Reply<T>>>,
    ) -> io::Result<Result<T, String>> {
        let request = self.next_request;
        self.next_request = request
            .checked_add(1)
            .ok_or_else(|| invalid("SSH terminal request IDs exhausted"))?;
        self.exchange_with_id(request, command, request_limit, response_limit, read_reply)
    }

    fn exchange_with_id<C: Serialize, T>(
        &mut self,
        request: u32,
        command: C,
        request_limit: usize,
        response_limit: usize,
        read_reply: impl FnOnce(&mut BufReader<DeadlineStream>, usize) -> io::Result<Option<Reply<T>>>,
    ) -> io::Result<Result<T, String>> {
        self.channel.get_mut().deadline = Instant::now() + REQUEST_TIMEOUT;
        write_record(
            self.channel.get_mut(),
            &Request {
                protocol: VERSION,
                request,
                host: self.host.clone(),
                command,
            },
            request_limit,
        )?;
        let reply = read_reply(&mut self.channel, response_limit)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "SSH terminal relay closed")
        })?;
        if reply.protocol != VERSION || reply.request != request {
            return Err(invalid("SSH terminal reply does not match the request"));
        }
        Ok(reply.result)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self
            .channel
            .get_ref()
            .channel
            .shutdown(std::net::Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_a_single_quoted_remote_command_and_refuses_option_like_hosts() {
        let endpoint = Endpoint::new(
            "ubuntu@192.0.2.10",
            Some(Path::new("/opt/aether's tools/host")),
            Some(Path::new("/run/user/1000/term.sock")),
        )
        .unwrap();
        assert_eq!(
            endpoint.remote_command,
            "exec '/opt/aether'\\''s tools/host' stdio '/run/user/1000/term.sock'"
        );
        for host in ["-F", "user@", "u@a@b", "a;sh", "u@h\n-q", "ssh://u@h"] {
            assert!(Endpoint::new(host, None, None).is_err(), "accepted {host}");
        }
        for host in ["::1", "_operator@2001:db8::10", "lab-terminal"] {
            assert!(Endpoint::new(host, None, None).is_ok(), "refused {host}");
        }
        assert!(Endpoint::new("machine", Some(Path::new("aether-terminal-host")), None).is_err());
    }

    #[test]
    fn an_expired_operation_cannot_read_or_write() {
        let (channel, _peer) = UnixStream::pair().unwrap();
        let mut stream = DeadlineStream {
            channel,
            deadline: Instant::now(),
        };
        assert_eq!(
            stream.read(&mut [0; 1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            stream.write(&[0; 1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
    }
}
