//! Native SSH terminal adapter. The WebView receives the existing v3 cell records.
mod catalog;

use self::catalog::{SavedShell, Store};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::Manager;
use tauri_terminal_frame_bridge::{
    semantic::{Attachment, SessionSelector},
    terminal_transport::{
        ssh::{Endpoint, Relay},
        valid_identity, Command, HOST_CHANGED,
    },
};

const NO_STORE: &str = "Saved SSH sessions are unavailable.";
const SESSION_UNAVAILABLE: &str = "terminal session unavailable";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Action {
    Open { destination: String },
    Attach,
    ListSaved,
    RestoreSaved { id: String },
    ForgetSaved { id: String },
    Request { command: Command },
}

#[derive(Default)]
pub(crate) struct SshTerminal {
    session: Option<RemoteSession>,
}

struct RemoteSession {
    endpoint: Endpoint,
    instance: Option<String>,
    saved_id: Option<String>,
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
    let store = window.path().app_data_dir().ok().map(Store::new);
    super::run_bridge(terminal.inner().clone(), move |terminal| {
        terminal.execute(action, store.as_ref())
    })
    .await
}

fn validate_owner(result: &Value) -> Result<Attachment, String> {
    let owner = serde_json::from_value::<Attachment>(result.clone())
        .map_err(|_| "Invalid SSH terminal attachment.")?;
    if !valid_identity(&owner.session) || owner.epoch == 0 || owner.generation == 0 {
        return Err("Invalid SSH terminal attachment.".into());
    }
    Ok(owner)
}

fn endpoint(destination: &str) -> Result<Endpoint, String> {
    let helper = std::env::var_os("AETHER_SSH_TERMINAL_HELPER").map(PathBuf::from);
    let socket = std::env::var_os("AETHER_SSH_TERMINAL_SOCKET").map(PathBuf::from);
    Endpoint::new(destination, helper.as_deref(), socket.as_deref())
}

fn connect(endpoint: &Endpoint, instance: Option<&str>) -> Result<Relay, String> {
    let mut relay = endpoint.connect().map_err(|_| "Unable to start OpenSSH.")?;
    if let Some(instance) = instance {
        relay.bind_host(Some(instance))?;
    }
    Ok(relay)
}

fn validate_snapshot(
    result: &Value,
    target: &SessionSelector,
    previous_generation: u32,
) -> Result<(Attachment, u32), String> {
    let next = validate_owner(&result["owner"])?;
    let frame = &result["frame"];
    let revision = frame["revision"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("Invalid SSH terminal snapshot revision.")?;
    if next.session != target.session
        || next.epoch != target.epoch
        || next.generation <= previous_generation
        || frame["version"] != 3
        || frame["session"].as_str() != Some(&next.session)
        || frame["epoch"] != next.epoch
    {
        return Err("Invalid SSH terminal recovery snapshot.".into());
    }
    Ok((next, revision))
}

impl SshTerminal {
    fn execute(&mut self, action: Action, store: Option<&Store>) -> Result<Value, String> {
        match action {
            Action::Open { destination } => {
                if self.session.is_some() {
                    return Err("Close the existing SSH terminal first.".into());
                }
                let endpoint = endpoint(&destination)?;
                let mut relay = connect(&endpoint, None)?;
                let instance = match relay.bind_host(None) {
                    Ok(instance) => Some(instance),
                    Err(_) => {
                        // An older helper closes an unfamiliar identity probe.
                        // Open is still sent only once.
                        relay = connect(&endpoint, None)?;
                        None
                    }
                };
                let opened = relay.call(Command::Open { rows: 24, cols: 80 })
                    .map_err(|_| "SSH terminal open is unconfirmed. Check the remote terminal service before opening another shell.")??;
                let owner = validate_owner(&opened)?;
                if owner.generation != 1 {
                    return Err("Invalid new SSH terminal generation.".into());
                }
                let remembered = match (store, instance.as_ref()) {
                    (Some(store), Some(instance)) => {
                        let shell = SavedShell::new(&endpoint, instance.clone(), &owner);
                        let id = shell.id.clone();
                        store.remember(shell).map(|()| id)
                    }
                    (None, _) => Err(NO_STORE.into()),
                    (_, None) => {
                        Err("Restart recovery requires a current SSH terminal helper.".into())
                    }
                };
                self.session = Some(RemoteSession {
                    endpoint,
                    instance,
                    saved_id: remembered.as_ref().ok().cloned(),
                    relay: Some(relay),
                    owner,
                    detached: false,
                    read_only: false,
                    input_uncertain: false,
                    recovered_from: None,
                    pending_snapshot: None,
                });
                Ok(json!({"owner": opened, "save_warning": remembered.err()}))
            }
            Action::ListSaved => store.ok_or(NO_STORE)?.list(),
            Action::RestoreSaved { id } => self.restore_saved(&id, store.ok_or(NO_STORE)?),
            Action::ForgetSaved { id } => {
                store.ok_or(NO_STORE)?.forget(&id)?;
                if let Some(session) = self.session.as_mut() {
                    if session.saved_id.as_deref() == Some(id.as_str()) {
                        session.saved_id = None;
                    }
                }
                Ok(Value::Null)
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
                    Err(reason) if reason == SESSION_UNAVAILABLE => {
                        let warning = session.forget_saved(store).err();
                        self.session = None;
                        if let Some(warning) = warning {
                            return Ok(json!({"ended":true,"message":warning}));
                        }
                        return Ok(Value::Null);
                    }
                    Err(reason) if reason == HOST_CHANGED => {
                        self.session = None;
                        return Ok(json!({
                            "ended": true,
                            "message": "The SSH terminal owner changed. Check the saved host before reconnecting.",
                        }));
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
                    Command::Identify | Command::Open { .. } | Command::AttachReadOnly { .. } => {
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
                        let mut result = match session.exchange(command, true) {
                            Err(reason) if reason == HOST_CHANGED => {
                                self.session = None;
                                return Ok(json!({
                                    "cleanup_pending": true,
                                    "save_warning": "The SSH terminal owner changed. Any saved entry was kept.",
                                }));
                            }
                            Err(reason) if reason == SESSION_UNAVAILABLE => {
                                json!({"cleanup_pending":true})
                            }
                            other => other?,
                        };
                        let receipt = result
                            .as_object_mut()
                            .ok_or("Invalid SSH terminal close receipt.")?;
                        if let Err(reason) = session.forget_saved(store) {
                            receipt.insert("save_warning".into(), Value::String(reason));
                        }
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

    fn restore_saved(&mut self, id: &str, store: &Store) -> Result<Value, String> {
        if self
            .session
            .as_ref()
            .is_some_and(|session| !session.detached && session.saved_id.as_deref() != Some(id))
        {
            return Err("Detach the current SSH terminal first.".into());
        }
        let saved = store.get(id)?;
        let endpoint = endpoint(&saved.destination)?;
        let attempt = (|| {
            let mut relay = connect(&endpoint, Some(&saved.instance))?;
            let result = relay
                .call(Command::AttachReadOnly {
                    session: saved.target.session.clone(),
                    epoch: saved.target.epoch,
                })
                .map_err(|_| "SSH terminal reconnection did not respond. Try again.")??;
            Ok::<_, String>((relay, result))
        })();
        let (relay, result) = match attempt {
            Err(reason) if reason == HOST_CHANGED => {
                self.retire_saved_view(id);
                return Err("The SSH terminal owner changed. Check the SSH host configuration or forget this saved entry.".into());
            }
            Err(reason) if reason == SESSION_UNAVAILABLE => {
                self.retire_saved_view(id);
                let message = "The saved SSH shell has expired or was closed.";
                let message = match store.forget(id) {
                    Ok(()) => message.to_owned(),
                    Err(reason) => format!("{message} {reason}"),
                };
                return Ok(json!({
                    "ended": true,
                    "message": message,
                }));
            }
            other => other?,
        };
        let generation = self
            .session
            .as_ref()
            .filter(|current| current.saved_id.as_deref() == Some(id))
            .map_or(1, |current| current.owner.generation);
        let (owner, revision) = validate_snapshot(&result, &saved.target, generation)?;
        self.session = Some(RemoteSession {
            endpoint,
            instance: Some(saved.instance),
            saved_id: Some(saved.id),
            relay: Some(relay),
            owner,
            detached: false,
            read_only: true,
            input_uncertain: false,
            recovered_from: None,
            pending_snapshot: Some(revision),
        });
        Ok(result)
    }

    fn retire_saved_view(&mut self, id: &str) {
        if self
            .session
            .as_ref()
            .and_then(|session| session.saved_id.as_deref())
            == Some(id)
        {
            self.session = None;
        }
    }
}

impl RemoteSession {
    fn forget_saved(&self, store: Option<&Store>) -> Result<(), String> {
        if let Some(id) = self.saved_id.as_deref() {
            store.ok_or(NO_STORE)?.forget(id)?;
        }
        Ok(())
    }

    fn input_allowed(&self) -> bool {
        !self.read_only && !self.input_uncertain && self.pending_snapshot.is_none()
    }

    fn install_snapshot(&mut self, result: &Value, previous: &Attachment) -> Result<(), String> {
        let target = SessionSelector {
            session: previous.session.clone(),
            epoch: previous.epoch,
        };
        let (next, revision) = validate_snapshot(result, &target, previous.generation)?;
        if next.generation < self.owner.generation {
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
        let host_allowed = match result.get("input_allowed") {
            Some(Value::Bool(allowed)) => *allowed,
            None => true, // Older helpers do not send an explicit grant.
            Some(_) => return Err("Invalid SSH terminal recovery input grant.".into()),
        };
        self.install_snapshot(&result, &previous)?;
        self.recovered_from = Some(previous);
        self.input_uncertain |= !host_allowed;
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
            self.relay = Some(connect(&self.endpoint, self.instance.as_deref())?);
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
