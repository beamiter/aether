//! Native SSH terminal adapter. The WebView receives the existing v3 cell records.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri_terminal_frame_bridge::{
    semantic::Attachment,
    terminal_transport::{
        ssh::{Endpoint, Relay},
        Command,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Action {
    Open { destination: String },
    Attach,
    Request { command: Command },
}

#[derive(Default)]
pub(crate) struct SshTerminal {
    session: Option<RemoteSession>,
}

struct RemoteSession {
    endpoint: Endpoint,
    relay: Option<Relay>,
    owner: Attachment,
    detached: bool,
    read_only: bool,
    input_uncertain: bool,
    recovered_from: Option<Attachment>,
    pending_snapshot: Option<u32>,
}

#[tauri::command]
pub(crate) async fn ssh_terminal<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    action: Action,
    terminal: tauri::State<'_, Arc<Mutex<SshTerminal>>>,
) -> Result<Value, String> {
    super::require_terminal_window(&window)?;
    super::run_bridge(terminal.inner().clone(), move |terminal| {
        terminal.execute(action)
    })
    .await
}

fn validate_owner(result: &Value) -> Result<Attachment, String> {
    let owner = serde_json::from_value::<Attachment>(result.clone())
        .map_err(|_| "Invalid SSH terminal attachment.")?;
    if owner.session.len() != 32
        || !owner
            .session
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || owner.epoch == 0
        || owner.generation == 0
    {
        return Err("Invalid SSH terminal attachment.".into());
    }
    Ok(owner)
}

impl SshTerminal {
    fn execute(&mut self, action: Action) -> Result<Value, String> {
        match action {
            Action::Open { destination } => {
                if self.session.is_some() {
                    return Err("Close the existing SSH terminal first.".into());
                }
                let helper = std::env::var_os("AETHER_SSH_TERMINAL_HELPER").map(PathBuf::from);
                let socket = std::env::var_os("AETHER_SSH_TERMINAL_SOCKET").map(PathBuf::from);
                let endpoint = Endpoint::new(&destination, helper.as_deref(), socket.as_deref())?;
                let mut relay = endpoint.connect().map_err(|_| "Unable to start OpenSSH.")?;
                let opened = relay.call(Command::Open { rows: 24, cols: 80 })
                    .map_err(|_| "SSH terminal did not respond. Check the host key, authentication and remote terminal service.")??;
                let owner = validate_owner(&opened)?;
                if owner.generation != 1 {
                    return Err("Invalid new SSH terminal generation.".into());
                }
                self.session = Some(RemoteSession {
                    endpoint,
                    relay: Some(relay),
                    owner,
                    detached: false,
                    read_only: false,
                    input_uncertain: false,
                    recovered_from: None,
                    pending_snapshot: None,
                });
                Ok(opened)
            }
            Action::Attach => {
                let Some(session) = self.session.as_mut() else {
                    return Ok(Value::Null);
                };
                if !session.detached {
                    return Err("Detach the current SSH view first.".into());
                }
                let previous = session.owner.clone();
                let result = match session.exchange(
                    Command::AttachReadOnly {
                        session: previous.session.clone(),
                        epoch: previous.epoch,
                    },
                    true,
                ) {
                    Err(reason) if reason == "terminal session unavailable" => {
                        self.session = None;
                        return Ok(Value::Null);
                    }
                    other => other?,
                };
                session.install_snapshot(&result, &previous)?;
                session.detached = false;
                session.read_only = true;
                session.recovered_from = None;
                Ok(result)
            }
            Action::Request { command } => {
                let session = self
                    .session
                    .as_mut()
                    .ok_or("No SSH terminal is connected.")?;
                if session.detached {
                    return Err("The SSH terminal view is detached.".into());
                }
                let owner = match &command {
                    Command::Open { .. } | Command::AttachReadOnly { .. } => {
                        return Err("Use the SSH terminal's open or attach action.".into());
                    }
                    Command::Recover { owner } => return session.recover(owner.clone()),
                    Command::Detach { owner }
                    | Command::Poll { owner }
                    | Command::Ack { owner, .. }
                    | Command::Status { owner }
                    | Command::History { owner, .. }
                    | Command::Write { owner, .. }
                    | Command::Key { owner, .. }
                    | Command::Text { owner, .. }
                    | Command::Paste { owner, .. }
                    | Command::Resize { owner, .. }
                    | Command::Close { owner } => owner,
                };
                if owner != &session.owner {
                    return Err("The SSH terminal attachment changed.".into());
                }
                if is_input(&command) && !session.input_allowed() {
                    return Err("SSH terminal input is unavailable for this attachment.".into());
                }
                match command {
                    Command::Detach { .. } => {
                        let result = session.exchange(command, false);
                        session.relay = None;
                        session.detached = true;
                        result
                    }
                    Command::Close { .. } => {
                        let result = match session.exchange(command, true) {
                            Err(reason) if reason == "terminal session unavailable" => {
                                json!({"cleanup_pending":true})
                            }
                            other => other?,
                        };
                        self.session = None;
                        Ok(result)
                    }
                    Command::Ack { revision, .. } => {
                        let result = session.exchange(command, false)?;
                        if session.pending_snapshot == Some(revision) {
                            session.pending_snapshot = None;
                            session.recovered_from = None;
                        }
                        Ok(result)
                    }
                    other => session.exchange(other, false),
                }
            }
        }
    }
}

impl RemoteSession {
    fn input_allowed(&self) -> bool {
        !self.read_only && !self.input_uncertain && self.pending_snapshot.is_none()
    }

    fn install_snapshot(&mut self, result: &Value, previous: &Attachment) -> Result<(), String> {
        let next = validate_owner(&result["owner"])?;
        let frame = &result["frame"];
        let revision = frame["revision"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("Invalid SSH terminal snapshot revision.")?;
        if next.session != previous.session
            || next.epoch != previous.epoch
            || next.generation <= previous.generation
            || next.generation < self.owner.generation
            || frame["version"] != 3
            || frame["session"].as_str() != Some(&next.session)
            || frame["epoch"] != next.epoch
        {
            return Err("Invalid SSH terminal recovery snapshot.".into());
        }
        self.owner = next;
        self.pending_snapshot = Some(revision);
        Ok(())
    }

    fn recover(&mut self, previous: Attachment) -> Result<Value, String> {
        if previous != self.owner && self.recovered_from.as_ref() != Some(&previous) {
            return Err("The SSH terminal attachment changed.".into());
        }
        let mut result = self.exchange(
            Command::Recover {
                owner: previous.clone(),
            },
            true,
        )?;
        self.install_snapshot(&result, &previous)?;
        self.recovered_from = Some(previous);
        result
            .as_object_mut()
            .ok_or("Invalid SSH terminal recovery result.")?
            .insert(
                "input_allowed".into(),
                Value::Bool(!self.read_only && !self.input_uncertain),
            );
        Ok(result)
    }

    fn exchange(&mut self, command: Command, reconnect: bool) -> Result<Value, String> {
        if reconnect && self.relay.is_none() {
            self.relay = Some(
                self.endpoint
                    .connect()
                    .map_err(|_| "Unable to start OpenSSH.")?,
            );
        }
        let relay = self
            .relay
            .as_mut()
            .ok_or("SSH terminal connection ended.")?;
        let input = is_input(&command);
        match relay.call(command) {
            Ok(reply) => reply,
            Err(_) => {
                self.relay = None;
                self.input_uncertain |= input;
                Err("SSH terminal connection ended.".into())
            }
        }
    }
}

fn is_input(command: &Command) -> bool {
    matches!(
        command,
        Command::Write { .. }
            | Command::Key { .. }
            | Command::Text { .. }
            | Command::Paste { .. }
            | Command::Resize { .. }
    )
}
