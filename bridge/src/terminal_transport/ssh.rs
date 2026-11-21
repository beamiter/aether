//! One authenticated OpenSSH relay, with an absolute deadline per RPC.
use super::{
    invalid, read_record, write_record, Command, Reply, Request, MAX_REPLY, MAX_REQUEST, VERSION,
};
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
            Some(path) => format!("exec {} stdio", quote_absolute_path(path)?),
            None => "exec aether-terminal-host stdio".to_string(),
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
}

impl Relay {
    /// An I/O error ends the relay. The caller can recover the known session
    /// on a new relay without retransmitting the interrupted operation.
    pub fn call(&mut self, command: Command) -> io::Result<Result<Value, String>> {
        let request = self.next_request;
        self.next_request = request
            .checked_add(1)
            .ok_or_else(|| invalid("SSH terminal request IDs exhausted"))?;
        self.channel.get_mut().deadline = Instant::now() + REQUEST_TIMEOUT;
        write_record(
            self.channel.get_mut(),
            &Request {
                protocol: VERSION,
                request,
                command,
            },
            MAX_REQUEST,
        )?;
        let reply: Reply = read_record(&mut self.channel, MAX_REPLY)?.ok_or_else(|| {
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
