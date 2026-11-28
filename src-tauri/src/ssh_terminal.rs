//! Native SSH terminal adapter for compatibility and packed terminal frames.
mod catalog;
mod reload;

use self::catalog::{SavedShell, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, value::RawValue, Value};
use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::Manager;
use tauri_terminal_frame_bridge::{
    protocol_v4::{
        Counter, Stamp, MAX_ENCODED_FRAME_BYTES, MAX_ENCODED_IMAGE_BYTES, RENDITION_VERSION,
    },
    semantic::{Attachment, SessionSelector},
    terminal_transport::{
        ssh::{Endpoint, Relay},
        valid_identity, Command, RawPackedResume, RawPollReply, HOST_CHANGED,
    },
    SSH_DENSE_FRAME_VERSION,
};

const NO_STORE: &str = "Saved SSH sessions are unavailable.";
const SESSION_UNAVAILABLE: &str = "terminal session unavailable";

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "operation", rename_all = "snake_case")]
pub(crate) enum Action {
    Open {
        destination: String,
    },
    Attach,
    PrepareReload {
        owner: Attachment,
        view: reload::View,
    },
    ClaimReload {
        ticket: String,
    },
    RecoverReloadLayout,
    ListSaved,
    RestoreSaved {
        id: String,
    },
    ForgetSaved {
        id: String,
    },
    Request {
        command: Command,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum Response {
    Json(Value),
    Frame(Box<RawValue>),
    Polled(RawPollReply),
    Packed {
        #[serde(flatten)]
        poll: RawPollReply,
        frame_bytes: usize,
    },
    Snapshot(SnapshotReply),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotReceipt {
    Compatibility(u32),
    Packed(Counter),
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum SnapshotReply {
    Compatibility(Value),
    Packed {
        #[serde(flatten)]
        receipt: RawPackedResume,
        frame_version: u8,
        frame_bytes: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        keyboard_event_reporting: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        view: Option<Value>,
    },
}

#[derive(Default)]
pub(crate) struct SshTerminal {
    session: Option<RemoteSession>,
    pending_reload: Option<reload::Pending>,
    retained_reload: Option<reload::RetainedLayout>,
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
    pending_snapshot: Option<SnapshotReceipt>,
    focus_reporting: bool,
    mouse_reporting: bool,
    keypad_reporting: bool,
    keyboard_event_reporting: bool,
    poll_with_status: bool,
    packed_frame_version: Option<u8>,
}

#[tauri::command]
pub(crate) async fn ssh_terminal<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    mut action: Action,
    terminal: tauri::State<'_, Arc<Mutex<SshTerminal>>>,
    keyboard: tauri::State<'_, Arc<Mutex<super::native_keyboard::Samples>>>,
) -> Result<Response, String> {
    super::require_terminal_window(&window)?;
    if let Action::Request {
        command: Command::KeyEvent { event, layout, .. },
    } = &mut action
    {
        *layout = keyboard
            .lock()
            .ok()
            .and_then(|mut keyboard| keyboard.take(event))
            .map(Into::into);
        event.native_time = None;
    }
    let label = window.label().to_owned();
    let store = window.path().app_data_dir().ok().map(Store::new);
    super::run_bridge(terminal.inner().clone(), move |terminal| {
        terminal.dispatch(action, store.as_ref(), &label)
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

impl SnapshotReply {
    fn packed(receipt: RawPackedResume) -> Result<Self, String> {
        let frame_bytes = receipt.frame.get().len();
        if frame_bytes > MAX_ENCODED_FRAME_BYTES + MAX_ENCODED_IMAGE_BYTES {
            return Err("SSH terminal frame exceeds its byte limit.".into());
        }
        Ok(Self::Packed {
            receipt,
            frame_version: SSH_DENSE_FRAME_VERSION,
            frame_bytes,
            keyboard_event_reporting: None,
            view: None,
        })
    }

    fn validate(
        &self,
        target: &SessionSelector,
        previous_generation: u32,
    ) -> Result<(Attachment, SnapshotReceipt), String> {
        match self {
            Self::Compatibility(result) => validate_snapshot(result, target, previous_generation)
                .map(|(owner, revision)| (owner, SnapshotReceipt::Compatibility(revision))),
            Self::Packed { receipt, .. } => {
                #[derive(Deserialize)]
                struct Header {
                    kind: String,
                    stamp: Stamp,
                }
                let invalid = "Invalid SSH terminal recovery snapshot.";
                let header: Header =
                    serde_json::from_str(receipt.frame.get()).map_err(|_| invalid)?;
                let next = &receipt.owner;
                if header.kind != "snapshot"
                    || header.stamp.version != SSH_DENSE_FRAME_VERSION
                    || header.stamp.session != target.session
                    || header.stamp.epoch.get() != u64::from(target.epoch)
                    || header.stamp.resource_generation != header.stamp.epoch
                    || next.session != target.session
                    || next.epoch != target.epoch
                    || next.generation <= previous_generation
                {
                    return Err(invalid.into());
                }
                Ok((next.clone(), SnapshotReceipt::Packed(header.stamp.revision)))
            }
        }
    }

    fn frame_version(&self) -> Option<u8> {
        match self {
            Self::Compatibility(_) => None,
            Self::Packed { frame_version, .. } => Some(*frame_version),
        }
    }

    fn input_grant(&self, explicit: bool) -> Result<bool, String> {
        match self {
            Self::Compatibility(result) => input_grant(result, explicit),
            Self::Packed { receipt, .. } => receipt
                .input_allowed
                .ok_or_else(|| "The SSH host did not explicitly grant input permission.".into()),
        }
    }

    fn keyboard_events_enabled(&self) -> bool {
        match self {
            Self::Compatibility(result) => {
                result.get("keyboard_events_enabled") == Some(&Value::Bool(true))
            }
            Self::Packed { receipt, .. } => receipt.keyboard_events_enabled == Some(true),
        }
    }

    fn set_recovery_grant(&mut self, allowed: bool, keyboard: bool) -> Result<(), String> {
        match self {
            Self::Compatibility(value) => {
                let receipt = value
                    .as_object_mut()
                    .ok_or("Invalid SSH terminal recovery result.")?;
                receipt.insert("input_allowed".into(), Value::Bool(allowed));
                receipt.insert("keyboard_event_reporting".into(), Value::Bool(keyboard));
            }
            Self::Packed {
                receipt,
                keyboard_event_reporting,
                ..
            } => {
                receipt.input_allowed = Some(allowed);
                *keyboard_event_reporting = Some(keyboard);
            }
        }
        Ok(())
    }

    fn set_view(&mut self, view: reload::View) -> Result<(), String> {
        let value = serde_json::to_value(view).map_err(|_| "Invalid retained layout.")?;
        match self {
            Self::Compatibility(result) => {
                result
                    .as_object_mut()
                    .ok_or("Invalid SSH handoff snapshot.")?
                    .insert("view".into(), value);
            }
            Self::Packed { view, .. } => *view = Some(value),
        }
        Ok(())
    }
}

fn attach_command(target: &SessionSelector, packed: bool) -> Command {
    let session = target.session.clone();
    let epoch = target.epoch;
    if packed {
        Command::AttachReadOnlyPacked { session, epoch }
    } else {
        Command::AttachReadOnly { session, epoch }
    }
}

fn read_snapshot(relay: &mut Relay, command: Command) -> io::Result<Result<SnapshotReply, String>> {
    if matches!(
        command,
        Command::RecoverPacked { .. } | Command::AttachReadOnlyPacked { .. }
    ) {
        Ok(relay.call_frame(command)?.and_then(SnapshotReply::packed))
    } else {
        Ok(relay.call(command)?.map(SnapshotReply::Compatibility))
    }
}

impl SshTerminal {
    fn dispatch(
        &mut self,
        action: Action,
        store: Option<&Store>,
        window: &str,
    ) -> Result<Response, String> {
        if self
            .pending_reload
            .as_ref()
            .is_some_and(|pending| pending.expired(std::time::Instant::now()))
        {
            self.retained_reload = self
                .pending_reload
                .take()
                .map(|pending| pending.retained_layout());
            if let Some(session) = self.session.as_mut() {
                session.read_only = true;
            }
        }
        if let Action::ClaimReload { ticket } = action {
            return self.claim_reload(&ticket, window).map(Response::Snapshot);
        }
        if matches!(action, Action::RecoverReloadLayout) {
            return self.recover_reload_layout(window).map(Response::Json);
        }
        if self.pending_reload.is_some() {
            return Err("The SSH writer is locked while its document is replaced.".into());
        }
        let response = match action {
            Action::PrepareReload { owner, view } => self.prepare_reload(owner, view, window),
            Action::ClaimReload { .. } | Action::RecoverReloadLayout => unreachable!(),
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
                let mut session = RemoteSession {
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
                    focus_reporting: false,
                    mouse_reporting: false,
                    keypad_reporting: false,
                    keyboard_event_reporting: false,
                    poll_with_status: false,
                    packed_frame_version: None,
                };
                session.negotiate_capabilities();
                let result = json!({
                    "owner": opened,
                    "save_warning": remembered.err(),
                    "keyboard_event_reporting": session.keyboard_event_reporting,
                    "poll_with_status": session.poll_with_status,
                    "packed_frame_version": session.packed_frame_version,
                });
                self.session = Some(session);
                Ok(result)
            }
            Action::ListSaved => store.ok_or(NO_STORE)?.list(),
            Action::RestoreSaved { id } => {
                return self
                    .restore_saved(&id, store.ok_or(NO_STORE)?)
                    .map(Response::Snapshot);
            }
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
                    return Ok(Response::Json(Value::Null));
                };
                if !session.detached {
                    return Err("Detach the current SSH view first.".into());
                }
                let previous = session.owner.clone();
                let target = SessionSelector {
                    session: previous.session.clone(),
                    epoch: previous.epoch,
                };
                let result = match session
                    .exchange_snapshot(|packed| attach_command(&target, packed))
                {
                    Err(reason) if reason == SESSION_UNAVAILABLE => {
                        let warning = session.forget_saved(store).err();
                        self.session = None;
                        self.retained_reload = None;
                        if let Some(warning) = warning {
                            return Ok(Response::Json(json!({"ended":true,"message":warning})));
                        }
                        return Ok(Response::Json(Value::Null));
                    }
                    Err(reason) if reason == HOST_CHANGED => {
                        self.session = None;
                        self.retained_reload = None;
                        return Ok(Response::Json(json!({
                            "ended": true,
                            "message": "The SSH terminal owner changed. Check the saved host before reconnecting.",
                        })));
                    }
                    other => other?,
                };
                session.install_snapshot(&result, &previous)?;
                session.detached = false;
                session.read_only = true;
                session.recovered_from = None;
                return Ok(Response::Snapshot(result));
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
                    Command::Identify
                    | Command::Open { .. }
                    | Command::AttachReadOnly { .. }
                    | Command::AttachReadOnlyPacked { .. } => {
                        return Err("Use the SSH terminal's open or attach action.".into());
                    }
                    Command::EnableKeyboardEvents { .. } => {
                        return Err("The SSH desktop negotiates keyboard reporting on open.".into());
                    }
                    Command::Recover { owner } => {
                        return session.recover(owner.clone()).map(Response::Snapshot);
                    }
                    Command::RecoverPacked { .. } => {
                        return Err(
                            "The SSH desktop negotiates the recovery snapshot format.".into()
                        );
                    }
                    Command::Detach { owner }
                    | Command::Poll { owner }
                    | Command::PollWithStatus { owner, .. }
                    | Command::PollPacked { owner, .. }
                    | Command::Ack { owner, .. }
                    | Command::AckPacked { owner, .. }
                    | Command::Status { owner }
                    | Command::History { owner, .. }
                    | Command::Write { owner, .. }
                    | Command::Mouse { owner, .. }
                    | Command::Focus { owner, .. }
                    | Command::Key { owner, .. }
                    | Command::KeyEvent { owner, .. }
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
                                self.retained_reload = None;
                                return Ok(Response::Json(json!({
                                    "cleanup_pending": true,
                                    "save_warning": "The SSH terminal owner changed. Any saved entry was kept.",
                                })));
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
                        self.retained_reload = None;
                        Ok(result)
                    }
                    Command::Ack { revision, .. } => {
                        session.acknowledge(command, SnapshotReceipt::Compatibility(revision))
                    }
                    Command::AckPacked { revision, .. } => {
                        session.acknowledge(command, SnapshotReceipt::Packed(revision))
                    }
                    Command::Poll { .. } => {
                        let frame = session
                            .exchange_with(command, false, Relay::call_frame)?
                            .ok_or("Invalid SSH terminal poll reply.")?;
                        return Ok(Response::Frame(frame));
                    }
                    Command::PollWithStatus { .. } | Command::PollPacked { .. } => {
                        let packed = matches!(command, Command::PollPacked { .. });
                        let mut reply: RawPollReply = session
                            .exchange_with(command, false, Relay::call_frame)?
                            .ok_or("Invalid SSH terminal poll reply.")?;
                        session.apply_status(&mut reply.status);
                        if packed {
                            let frame_bytes = reply.update.get().len();
                            if frame_bytes > MAX_ENCODED_FRAME_BYTES + MAX_ENCODED_IMAGE_BYTES {
                                return Err("SSH terminal frame exceeds its byte limit.".into());
                            }
                            return Ok(Response::Packed {
                                poll: reply,
                                frame_bytes,
                            });
                        }
                        return Ok(Response::Polled(reply));
                    }
                    other => session.exchange(other, false),
                }
            }
        };
        response.map(Response::Json)
    }

    fn prepare_reload(
        &mut self,
        owner: Attachment,
        view: reload::View,
        window: &str,
    ) -> Result<Value, String> {
        view.validate()?;
        let session = self
            .session
            .as_mut()
            .ok_or("No SSH terminal is connected.")?;
        if session.detached || owner != session.owner || !session.input_allowed() {
            return Err(
                "Only the current acknowledged SSH writer can replace its document.".into(),
            );
        }
        // Advance at the host before exposing the ticket. Old document input
        // cannot race a reload or regain authority through an idempotent recover.
        let recovered = match session.recover_with_grant(owner, true) {
            Ok(result) => result,
            Err(error) => {
                session.read_only = true;
                return Err(error);
            }
        };
        if recovered.input_grant(true) != Ok(true) {
            session.read_only = true;
            return Err("The SSH host did not renew input permission.".into());
        }
        let pending = reload::Pending::new(
            session.owner.clone(),
            view,
            window,
            std::time::Instant::now(),
        );
        let result = json!({"ticket": pending.ticket});
        self.retained_reload = None;
        self.pending_reload = Some(pending);
        Ok(result)
    }

    fn claim_reload(&mut self, ticket: &str, window: &str) -> Result<SnapshotReply, String> {
        // Take before any network operation; failures and duplicate claims never
        // replay a writer grant. Saved-session restore remains a separate reader.
        let pending = self
            .pending_reload
            .take()
            .ok_or("No one-use SSH writer handoff is available.")?;
        let session = self
            .session
            .as_mut()
            .ok_or("No SSH terminal is connected.")?;
        if let Err(error) = pending.authorize(
            ticket,
            window,
            &session.owner,
            session.read_only,
            session.input_uncertain,
            std::time::Instant::now(),
        ) {
            session.read_only = true;
            self.retained_reload = Some(pending.retained_layout());
            return Err(error);
        }
        self.retained_reload = Some(pending.retained_layout());
        match session.recover_with_grant(pending.owner, true) {
            Ok(mut result) => {
                result.set_view(pending.view)?;
                Ok(result)
            }
            Err(error) => {
                session.read_only = true;
                Err(error)
            }
        }
    }

    fn recover_reload_layout(&mut self, window: &str) -> Result<Value, String> {
        if self
            .pending_reload
            .as_ref()
            .is_some_and(|pending| !pending.belongs_to(window))
        {
            return Err("The retained layout belongs to another window.".into());
        }
        // An orphaned/malformed ticket cannot regain writer authority. Preserve
        // only reader metadata; no host attach, process launch or input occurs.
        if let Some(pending) = self.pending_reload.take() {
            self.retained_reload = Some(pending.retained_layout());
            if let Some(session) = self.session.as_mut() {
                session.read_only = true;
            }
        }
        let Some(retained) = self.retained_reload.take() else {
            return Ok(Value::Null);
        };
        let session = self
            .session
            .as_mut()
            .ok_or("The original SSH actor is unavailable.")?;
        if retained.matches(window, &session.owner) {
            session.read_only = true;
        }
        let view = retained.take(window, &session.owner, std::time::Instant::now())?;
        Ok(json!({"view": view}))
    }

    fn restore_saved(&mut self, id: &str, store: &Store) -> Result<SnapshotReply, String> {
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
            let command = attach_command(&saved.target, relay.supports_packed_snapshots());
            let result = read_snapshot(&mut relay, command)
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
                return Ok(SnapshotReply::Compatibility(json!({
                    "ended": true,
                    "message": message,
                })));
            }
            other => other?,
        };
        let generation = self
            .session
            .as_ref()
            .filter(|current| current.saved_id.as_deref() == Some(id))
            .map_or(1, |current| current.owner.generation);
        let (owner, revision) = result.validate(&saved.target, generation)?;
        let packed_frame_version = result.frame_version();
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
            focus_reporting: false,
            mouse_reporting: false,
            keypad_reporting: false,
            keyboard_event_reporting: false,
            poll_with_status: packed_frame_version.is_some(),
            packed_frame_version,
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
            self.retained_reload = None;
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

    fn negotiate_capabilities(&mut self) {
        if !self.instance.as_deref().is_some_and(valid_identity) {
            return;
        }
        let owner = self.owner.clone();
        let supported = self
            .exchange(
                Command::Status {
                    owner: owner.clone(),
                },
                false,
            )
            .is_ok_and(|status| {
                status.get("keyboard_events_supported") == Some(&Value::Bool(true))
            });
        self.keyboard_event_reporting = supported
            && self.exchange(Command::EnableKeyboardEvents { owner }, false)
                == Ok(Value::Bool(true));
    }

    fn install_snapshot(
        &mut self,
        result: &SnapshotReply,
        previous: &Attachment,
    ) -> Result<(), String> {
        let target = SessionSelector {
            session: previous.session.clone(),
            epoch: previous.epoch,
        };
        let (next, revision) = result.validate(&target, previous.generation)?;
        if next.generation < self.owner.generation {
            return Err("Invalid SSH terminal recovery snapshot.".into());
        }
        self.owner = next;
        self.clear_capabilities();
        self.packed_frame_version = result.frame_version();
        self.poll_with_status = self.packed_frame_version.is_some();
        self.pending_snapshot = Some(revision);
        Ok(())
    }

    fn recover(&mut self, previous: Attachment) -> Result<SnapshotReply, String> {
        self.recover_with_grant(previous, false)
    }

    fn recover_with_grant(
        &mut self,
        previous: Attachment,
        explicit: bool,
    ) -> Result<SnapshotReply, String> {
        if previous != self.owner && self.recovered_from.as_ref() != Some(&previous) {
            return Err("The SSH terminal attachment changed.".into());
        }
        let mut result = self.exchange_snapshot(|packed| {
            let owner = previous.clone();
            if packed {
                Command::RecoverPacked { owner }
            } else {
                Command::Recover { owner }
            }
        })?;
        let host_allowed = result.input_grant(explicit)?;
        self.install_snapshot(&result, &previous)?;
        self.recovered_from = Some(previous);
        self.input_uncertain |= !host_allowed;
        let allowed = !self.read_only && !self.input_uncertain;
        self.keyboard_event_reporting = allowed
            && self.instance.as_deref().is_some_and(valid_identity)
            && result.keyboard_events_enabled();
        result.set_recovery_grant(allowed, self.keyboard_event_reporting)?;
        Ok(result)
    }

    fn acknowledge(&mut self, command: Command, receipt: SnapshotReceipt) -> Result<Value, String> {
        if self
            .pending_snapshot
            .is_some_and(|pending| pending != receipt)
        {
            return Err("SSH terminal snapshot acknowledgment does not match.".into());
        }
        if matches!(receipt, SnapshotReceipt::Packed(_))
            && !self
                .relay
                .as_ref()
                .is_some_and(Relay::supports_packed_snapshots)
        {
            return Err("SSH terminal packed snapshot acknowledgment is unavailable.".into());
        }
        let result = self.exchange(command, false)?;
        if self.pending_snapshot == Some(receipt) {
            self.pending_snapshot = None;
            self.recovered_from = None;
        }
        Ok(result)
    }

    fn clear_capabilities(&mut self) {
        self.focus_reporting = false;
        self.mouse_reporting = false;
        self.keypad_reporting = false;
        self.keyboard_event_reporting = false;
        self.poll_with_status = false;
        self.packed_frame_version = None;
    }

    fn ensure_relay(&mut self) -> Result<(), String> {
        if self.relay.is_none() {
            self.clear_capabilities();
            self.relay = Some(connect(&self.endpoint, self.instance.as_deref())?);
        }
        Ok(())
    }

    fn exchange_snapshot(
        &mut self,
        command: impl FnOnce(bool) -> Command,
    ) -> Result<SnapshotReply, String> {
        self.ensure_relay()?;
        let packed = self
            .relay
            .as_ref()
            .is_some_and(Relay::supports_packed_snapshots);
        self.exchange_with(command(packed), false, read_snapshot)?
            .ok_or_else(|| "Invalid SSH terminal recovery result.".into())
    }

    fn exchange(&mut self, command: Command, reconnect: bool) -> Result<Value, String> {
        let status = matches!(command, Command::Status { .. });
        let mut reply = self
            .exchange_with(command, reconnect, Relay::call)?
            .unwrap_or(Value::Null);
        if status {
            self.packed_frame_version = packed_frame_capability(self.instance.as_deref(), &reply);
            self.apply_status(&mut reply);
        }
        Ok(reply)
    }

    fn exchange_with<T>(
        &mut self,
        mut command: Command,
        reconnect: bool,
        call: impl FnOnce(&mut Relay, Command) -> io::Result<Result<T, String>>,
    ) -> Result<Option<T>, String> {
        if self.relay.is_none() {
            self.clear_capabilities();
        }
        // Optional input commands require capability from this bound host
        // and a current writer attachment.
        if matches!(command, Command::Focus { .. })
            && (!self.focus_reporting
                || !self.input_allowed()
                || self.detached
                || !self.instance.as_deref().is_some_and(valid_identity))
        {
            return Err("SSH terminal focus reporting is unavailable.".into());
        }

        if matches!(command, Command::Mouse { .. })
            && (!self.mouse_reporting
                || !self.input_allowed()
                || self.detached
                || !self.instance.as_deref().is_some_and(valid_identity))
        {
            return Err("SSH terminal mouse reporting is unavailable.".into());
        }

        if matches!(command, Command::KeyEvent { .. }) && !self.keyboard_event_reporting {
            return Err("SSH terminal physical keyboard reporting is unavailable.".into());
        }

        let batched_poll = matches!(
            command,
            Command::PollWithStatus { .. } | Command::PollPacked { .. }
        );
        if batched_poll && (!self.poll_with_status || self.pending_snapshot.is_some()) {
            return Err("SSH terminal batched polling is unavailable for this attachment.".into());
        }
        if let Command::PollPacked { frame_version, .. } = &command {
            if self.packed_frame_version != Some(frame_version.unwrap_or(RENDITION_VERSION)) {
                return Err(
                    "SSH terminal packed frame version is unavailable for this attachment.".into(),
                );
            }
        }

        if reconnect {
            self.ensure_relay()?;
        }
        let input =
            is_input(&command) && !matches!(command, Command::Focus { .. } | Command::Mouse { .. });
        if matches!(
            command,
            Command::Recover { .. } | Command::RecoverPacked { .. } | Command::Detach { .. }
        ) {
            self.clear_capabilities();
        }
        let relay = self
            .relay
            .as_mut()
            .ok_or("SSH terminal connection ended.")?;
        if !self.keypad_reporting && !strip_unavailable_keypad(&mut command) {
            return Ok(None);
        }
        match call(relay, command) {
            Ok(Ok(reply)) => Ok(Some(reply)),
            Ok(Err(error)) => {
                self.focus_reporting = false;
                self.mouse_reporting = false;
                self.keypad_reporting = false;
                Err(error)
            }
            Err(_) => {
                self.relay = None;
                self.clear_capabilities();
                self.input_uncertain |= input;
                Err("SSH terminal connection ended.".into())
            }
        }
    }

    fn apply_status(&mut self, status: &mut Value) {
        let instance = self.instance.as_deref();
        self.focus_reporting = focus_capability(instance, status);
        self.mouse_reporting = mouse_capability(instance, status);
        self.keypad_reporting = keypad_capability(instance, status);
        self.poll_with_status = instance.is_some_and(valid_identity)
            && status.get("poll_with_status") == Some(&Value::Bool(true));
        if let Some(status) = status.as_object_mut() {
            status.remove("packed_frame_relay");
            status.remove("packed_frame_relay_version");
            status.remove("dense_frame_version");
            if let Some(version) = self.packed_frame_version.filter(|_| self.poll_with_status) {
                status.insert("packed_frame_version".into(), version.into());
            } else {
                status.remove("packed_frame_version");
            }
            status.insert("focus_reporting".into(), Value::Bool(self.focus_reporting));
            status.insert("mouse_reporting".into(), Value::Bool(self.mouse_reporting));
            status.insert(
                "poll_with_status".into(),
                Value::Bool(self.poll_with_status),
            );
            if !self.mouse_reporting {
                status.remove("mouse");
                status.remove("alternate_scroll");
            }
        }
    }
}

fn packed_frame_capability(instance: Option<&str>, status: &Value) -> Option<u8> {
    if !instance.is_some_and(valid_identity)
        || status.get("poll_with_status") != Some(&Value::Bool(true))
        || status.get("packed_frame_relay") != Some(&Value::Bool(true))
        || status.get("packed_frame_version").and_then(Value::as_u64)
            != Some(u64::from(RENDITION_VERSION))
    {
        return None;
    }
    let dense = Some(u64::from(SSH_DENSE_FRAME_VERSION));
    if status.get("dense_frame_version").and_then(Value::as_u64) == dense
        && status
            .get("packed_frame_relay_version")
            .and_then(Value::as_u64)
            == dense
    {
        Some(SSH_DENSE_FRAME_VERSION)
    } else {
        Some(RENDITION_VERSION)
    }
}

fn keypad_capability(instance: Option<&str>, status: &Value) -> bool {
    instance.is_some_and(valid_identity)
        && status.get("keypad_reporting") == Some(&Value::Bool(true))
}

fn strip_unavailable_keypad(command: &mut Command) -> bool {
    if let Command::Key { name, keypad, .. } = command {
        // The old logical-key path excludes the keypad center's Clear name.
        return keypad.take().is_none() || name != "Clear";
    }
    true
}

fn mouse_capability(instance: Option<&str>, status: &Value) -> bool {
    instance.is_some_and(valid_identity)
        && status.get("mouse_reporting") == Some(&Value::Bool(true))
}

fn focus_capability(instance: Option<&str>, status: &Value) -> bool {
    instance.is_some_and(valid_identity)
        && status.get("focus_reporting") == Some(&Value::Bool(true))
}

fn input_grant(result: &Value, explicit: bool) -> Result<bool, String> {
    match result.get("input_allowed") {
        Some(Value::Bool(allowed)) => Ok(*allowed),
        None if !explicit => Ok(true), // Existing legacy recovery compatibility only.
        None => Err("The SSH host did not explicitly grant input permission.".into()),
        Some(_) => Err("Invalid SSH terminal recovery input grant.".into()),
    }
}

fn is_input(command: &Command) -> bool {
    matches!(
        command,
        Command::Write { .. }
            | Command::Mouse { .. }
            | Command::Focus { .. }
            | Command::Key { .. }
            | Command::KeyEvent { .. }
            | Command::Text { .. }
            | Command::Paste { .. }
            | Command::Resize { .. }
    )
}

#[cfg(test)]
mod reload_tests {
    use super::*;
    use tauri::ipc::{InvokeResponseBody, IpcResponse};

    impl SshTerminal {
        fn execute(
            &mut self,
            action: Action,
            store: Option<&Store>,
            window: &str,
        ) -> Result<Value, String> {
            let reply = self.dispatch(action, store, window)?;
            serde_json::to_value(reply).map_err(|error| error.to_string())
        }
    }

    #[test]
    fn packed_snapshot_validation_keeps_the_full_receipt_and_blocks_compatibility_ack() {
        let mut terminal = terminal(Duration::ZERO);
        let session = terminal.session.as_mut().unwrap();
        let old = session.owner.clone();
        let next = Attachment {
            generation: old.generation + 1,
            ..old.clone()
        };
        let revision = Counter::new(9_007_199_254_740_993).unwrap();
        let snapshot = |kind| {
            let frame = serde_json::value::to_raw_value(&json!({
                "kind":kind,"stamp":{
                    "version":SSH_DENSE_FRAME_VERSION,"session":&old.session,"epoch":old.epoch.to_string(),
                    "revision":revision,"layout":"1","screen":"primary","resource_generation":old.epoch.to_string(),
                    "cursor_row":0,"cursor_col":0,"cursor_visible":true,"cursor_shape":"block","pending_wrap":false
                }
            })).unwrap();
            SnapshotReply::packed(RawPackedResume {
                owner: next.clone(),
                frame,
                input_allowed: Some(true),
                keyboard_events_enabled: None,
            })
            .unwrap()
        };
        assert!(session.install_snapshot(&snapshot("delta"), &old).is_err());
        assert_eq!(session.owner, old);
        session
            .install_snapshot(&snapshot("snapshot"), &old)
            .unwrap();
        assert_eq!(
            session.pending_snapshot,
            Some(SnapshotReceipt::Packed(revision))
        );
        assert_eq!(session.packed_frame_version, Some(SSH_DENSE_FRAME_VERSION));
        assert!(!session.input_allowed());
        let bad_ack = Command::Ack {
            owner: next,
            revision: 1,
        };
        assert_eq!(
            session
                .acknowledge(bad_ack, SnapshotReceipt::Compatibility(1))
                .unwrap_err(),
            "SSH terminal snapshot acknowledgment does not match."
        );
        assert_eq!(
            session.pending_snapshot,
            Some(SnapshotReceipt::Packed(revision))
        );
    }

    #[test]
    fn packed_ipc_contains_the_frame_object_and_its_byte_length() {
        let frame = r#"{ "kind":"snapshot","stamp":{"revision":"8"},"sample":"雪" }"#;
        let poll: RawPollReply = serde_json::from_str(&format!(
            r#"{{"update":{frame},"status":{{"epoch":4,"packed_frame_version":8}}}}"#
        ))
        .unwrap();
        let InvokeResponseBody::Json(wire) = (Response::Packed {
            frame_bytes: poll.update.get().len(),
            poll,
        })
        .body()
        .unwrap() else {
            panic!("terminal IPC should return JSON");
        };
        assert!(wire.contains(frame));
        let reply: Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(reply["frame_bytes"], frame.len());
        assert_eq!(reply["update"]["stamp"]["revision"], "8");
        assert_eq!(reply["status"]["packed_frame_version"], RENDITION_VERSION);
    }

    #[test]
    fn polled_frames_remain_json_objects_at_the_webview_boundary() {
        let frame = r#"{ "kind":"delta", "frame":{"revision":9,"row":"雪"} }"#;
        let mut reply: RawPollReply =
            serde_json::from_str(&format!(r#"{{"update":{frame},"status":{{"epoch":4}}}}"#))
                .unwrap();
        reply.status["poll_with_status"] = Value::Bool(true);
        let InvokeResponseBody::Json(wire) = Response::Polled(reply).body().unwrap() else {
            panic!("terminal IPC should return JSON");
        };
        assert!(wire.contains(frame));
        let decoded: Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded["update"]["frame"]["revision"], 9);
        assert_eq!(decoded["status"]["poll_with_status"], true);

        let null = serde_json::from_str("null").unwrap();
        let InvokeResponseBody::Json(wire) = Response::Frame(null).body().unwrap() else {
            panic!("terminal IPC should return JSON");
        };
        assert_eq!(wire, "null");
    }

    use std::time::{Duration, Instant};
    fn owner() -> Attachment {
        Attachment {
            session: "a".repeat(32),
            epoch: 4,
            generation: 7,
        }
    }
    fn terminal(age: Duration) -> SshTerminal {
        let view=serde_json::from_value(json!({"version":1,"tabs":[],"splitEnabled":false,"splitEpochs":[],"detachedLocal":null,"remoteDraft":""})).unwrap();
        SshTerminal {
            retained_reload: None,
            pending_reload: Some(reload::Pending::new(
                owner(),
                view,
                "main",
                Instant::now() - age,
            )),
            session: Some(RemoteSession {
                endpoint: Endpoint::new("localhost", None, None).unwrap(),
                instance: None,
                saved_id: None,
                relay: None,
                owner: owner(),
                detached: false,
                read_only: false,
                input_uncertain: false,
                recovered_from: None,
                pending_snapshot: None,
                focus_reporting: false,
                mouse_reporting: false,
                keypad_reporting: false,
                keyboard_event_reporting: false,
                poll_with_status: false,
                packed_frame_version: None,
            }),
        }
    }

    #[test]
    fn mouse_capability_requires_bound_current_status_before_any_command() {
        let identity = "a".repeat(32);
        for status in [
            json!({}),
            json!({"mouse_reporting":false}),
            json!({"mouse_reporting":"true"}),
        ] {
            assert!(!mouse_capability(Some(&identity), &status));
        }
        assert!(!mouse_capability(None, &json!({"mouse_reporting":true})));
        assert!(mouse_capability(
            Some(&identity),
            &json!({"mouse_reporting":true})
        ));
        let mut t = terminal(Duration::ZERO);
        t.pending_reload = None;
        let report=serde_json::from_value(json!({"layout":"1","revision":"1","generation":1,"row":0,"col":0,"kind":"press","button":"left","alt":false,"ctrl":false})).unwrap();
        let result = t.execute(
            Action::Request {
                command: Command::Mouse {
                    owner: owner(),
                    report,
                },
            },
            None,
            "main",
        );
        assert_eq!(
            result.unwrap_err(),
            "SSH terminal mouse reporting is unavailable."
        );
        assert!(t.session.as_ref().unwrap().relay.is_none());
        assert!(!t.session.as_ref().unwrap().input_uncertain);
    }

    #[test]
    fn keypad_fields_are_sent_only_to_a_capable_bound_helper() {
        let mut command: Command = serde_json::from_value(json!({
            "kind": "key", "owner": owner(), "name": "Home",
            "shift": false, "alt": false, "ctrl": false,
            "keypad": {"code": "Numpad7", "num_locked": false}
        }))
        .unwrap();
        assert!(!keypad_capability(None, &json!({"keypad_reporting":true})));
        assert!(!keypad_capability(Some(&"a".repeat(32)), &json!({})));
        assert!(keypad_capability(
            Some(&"a".repeat(32)),
            &json!({"keypad_reporting":true})
        ));
        assert!(serde_json::to_value(&command)
            .unwrap()
            .get("keypad")
            .is_some());
        assert!(strip_unavailable_keypad(&mut command));
        let old_wire = serde_json::to_value(&command).unwrap();
        assert_eq!(old_wire["name"], "Home");
        assert!(old_wire.get("keypad").is_none());

        let mut center: Command = serde_json::from_value(json!({
            "kind": "key", "owner": owner(), "name": "Clear",
            "shift": false, "alt": false, "ctrl": false,
            "keypad": {"code": "Numpad5", "num_locked": false}
        }))
        .unwrap();
        assert!(!strip_unavailable_keypad(&mut center));
    }

    #[test]
    fn packed_poll_requires_the_bound_owner_version_and_stdio_capability() {
        let identity = "a".repeat(32);
        let status = json!({
            "poll_with_status": true,
            "packed_frame_version": RENDITION_VERSION,
            "packed_frame_relay": true,
        });
        assert_eq!(
            packed_frame_capability(Some(&identity), &status),
            Some(RENDITION_VERSION)
        );
        assert_eq!(packed_frame_capability(None, &status), None);
        let mut old_stdio = status.clone();
        old_stdio
            .as_object_mut()
            .unwrap()
            .remove("packed_frame_relay");
        assert_eq!(packed_frame_capability(Some(&identity), &old_stdio), None);
        let mut old_owner = status.clone();
        old_owner
            .as_object_mut()
            .unwrap()
            .remove("packed_frame_version");
        assert_eq!(packed_frame_capability(Some(&identity), &old_owner), None);
        let mut unknown_version = status.clone();
        unknown_version["packed_frame_version"] = json!("8");
        assert_eq!(
            packed_frame_capability(Some(&identity), &unknown_version),
            None
        );

        let mut dense = status.clone();
        dense["dense_frame_version"] = SSH_DENSE_FRAME_VERSION.into();
        assert_eq!(
            packed_frame_capability(Some(&identity), &dense),
            Some(RENDITION_VERSION)
        );
        dense["packed_frame_relay_version"] = SSH_DENSE_FRAME_VERSION.into();
        assert_eq!(
            packed_frame_capability(Some(&identity), &dense),
            Some(SSH_DENSE_FRAME_VERSION)
        );
        dense.as_object_mut().unwrap().remove("dense_frame_version");
        assert_eq!(
            packed_frame_capability(Some(&identity), &dense),
            Some(RENDITION_VERSION)
        );

        let mut terminal = terminal(Duration::ZERO);
        let session = terminal.session.as_mut().unwrap();
        session.instance = Some(identity);
        session.apply_status(&mut old_stdio);
        assert!(old_stdio.get("packed_frame_version").is_none());
        session.packed_frame_version = Some(RENDITION_VERSION);
        let mut negotiated = status;
        session.apply_status(&mut negotiated);
        assert_eq!(negotiated["packed_frame_version"], RENDITION_VERSION);
        assert!(negotiated.get("packed_frame_relay").is_none());
    }

    #[test]
    fn batched_poll_requires_a_bound_host_with_an_explicit_capability() {
        let mut terminal = terminal(Duration::ZERO);
        let session = terminal.session.as_mut().unwrap();
        session.apply_status(&mut json!({"poll_with_status":true}));
        assert!(!session.poll_with_status);
        session.instance = Some("a".repeat(32));
        for mut status in [json!({}), json!({"poll_with_status":"true"})] {
            session.apply_status(&mut status);
            assert!(!session.poll_with_status);
            assert_eq!(status["poll_with_status"], false);
        }
        session.apply_status(&mut json!({"poll_with_status":true}));
        assert!(session.poll_with_status);

        // A lost link retires the advertisement for this attachment.
        let result = session.exchange(
            Command::PollWithStatus {
                owner: owner(),
                ack_revision: None,
            },
            false,
        );
        assert!(result
            .unwrap_err()
            .contains("batched polling is unavailable"));
        assert!(!session.poll_with_status);
        assert!(!session.input_uncertain);
    }

    #[test]
    fn focus_capability_requires_current_bound_status_and_never_probes_old_hosts() {
        assert!(!focus_capability(
            Some(&"a".repeat(32)),
            &json!({"exited":false})
        ));
        assert!(!focus_capability(None, &json!({"focus_reporting":true})));
        assert!(!focus_capability(
            Some("bad"),
            &json!({"focus_reporting":true})
        ));
        assert!(!focus_capability(
            Some(&"a".repeat(32)),
            &json!({"focus_reporting":"true"})
        ));
        assert!(focus_capability(
            Some(&"a".repeat(32)),
            &json!({"focus_reporting":true})
        ));
        let mut t = terminal(Duration::ZERO);
        let session = t.session.as_mut().unwrap();
        let command = || Command::Focus {
            owner: owner(),
            focused: true,
        };
        // No relay exists: unsupported focus must fail at the capability gate,
        // before any attempt to reconnect or send an unfamiliar command.
        assert_eq!(
            session.exchange(command(), false).unwrap_err(),
            "SSH terminal focus reporting is unavailable."
        );
        assert!(!session.input_uncertain);
        session.focus_reporting = true;
        session.read_only = true;
        assert!(session
            .exchange(command(), false)
            .unwrap_err()
            .contains("focus reporting"));
        session.read_only = false;
        session.pending_snapshot = Some(SnapshotReceipt::Compatibility(1));
        assert!(session
            .exchange(command(), false)
            .unwrap_err()
            .contains("focus reporting"));
    }

    #[test]
    fn ssh_reload_pending_blocks_old_input_and_wrong_claim_consumes_without_network() {
        let mut t = terminal(Duration::ZERO);
        assert!(t
            .execute(
                Action::Request {
                    command: Command::Text {
                        owner: owner(),
                        text: "forbidden".into()
                    }
                },
                None,
                "main"
            )
            .is_err());
        assert!(t.execute(Action::Attach, None, "main").is_err());
        assert!(t
            .execute(
                Action::ClaimReload {
                    ticket: "wrong".into()
                },
                None,
                "main"
            )
            .is_err());
        assert!(t.pending_reload.is_none());
        assert!(t.session.as_ref().unwrap().read_only);
        assert!(t
            .execute(
                Action::ClaimReload {
                    ticket: "wrong".into()
                },
                None,
                "main"
            )
            .is_err());
        assert!(t.session.as_ref().unwrap().relay.is_none());
    }
    #[test]
    fn ssh_reload_expiry_and_foreign_generation_keep_writer_closed() {
        let mut t = terminal(Duration::from_secs(31));
        assert!(t
            .execute(
                Action::Request {
                    command: Command::Text {
                        owner: owner(),
                        text: "forbidden".into()
                    }
                },
                None,
                "main"
            )
            .is_err());
        assert!(t.pending_reload.is_none());
        assert!(t.session.as_ref().unwrap().read_only);
        let mut t = terminal(Duration::ZERO);
        let ticket = t.pending_reload.as_ref().unwrap().ticket.clone();
        t.session.as_mut().unwrap().owner.generation += 1;
        assert!(t
            .execute(Action::ClaimReload { ticket }, None, "main")
            .is_err());
        assert!(t.session.as_ref().unwrap().read_only);
        assert!(t.session.as_ref().unwrap().relay.is_none());
    }
    #[test]
    fn ssh_reload_invalid_expired_and_orphaned_handoffs_recover_metadata_once_without_input() {
        for mode in ["invalid", "expired", "orphan"] {
            let mut t = terminal(if mode == "expired" {
                Duration::from_secs(31)
            } else {
                Duration::ZERO
            });
            let ticket = t.pending_reload.as_ref().unwrap().ticket.clone();
            if mode != "orphan" {
                assert!(t
                    .execute(
                        Action::ClaimReload {
                            ticket: if mode == "invalid" {
                                "wrong".into()
                            } else {
                                ticket
                            }
                        },
                        None,
                        "main"
                    )
                    .is_err());
            }
            let result = t
                .execute(Action::RecoverReloadLayout, None, "main")
                .unwrap();
            assert!(result.get("view").is_some());
            assert!(result.get("input_allowed").is_none());
            assert!(result.get("owner").is_none());
            assert_eq!(result["view"]["remoteDraft"], "");
            assert!(t.session.as_ref().unwrap().read_only);
            assert!(t.session.as_ref().unwrap().relay.is_none());
            assert!(t
                .execute(Action::RecoverReloadLayout, None, "main")
                .unwrap()
                .is_null());
            assert!(t
                .execute(
                    Action::Request {
                        command: Command::Text {
                            owner: owner(),
                            text: "forbidden".into()
                        }
                    },
                    None,
                    "main"
                )
                .is_err());
        }
    }
    #[test]
    fn ssh_reload_metadata_cannot_cross_window_or_ssh_actor() {
        let mut t = terminal(Duration::ZERO);
        assert!(t
            .execute(Action::RecoverReloadLayout, None, "other")
            .is_err());
        assert!(t.pending_reload.is_some());
        let mut t = terminal(Duration::from_secs(31));
        t.session.as_mut().unwrap().owner.session = "c".repeat(32);
        assert!(t
            .execute(Action::RecoverReloadLayout, None, "main")
            .is_err());
        assert!(t.retained_reload.is_none());
    }
    #[test]
    fn ssh_reload_requires_explicit_host_grant_without_changing_legacy_recover() {
        assert!(input_grant(&json!({}), false).unwrap());
        assert!(input_grant(&json!({}), true).is_err());
        assert!(input_grant(&json!({"input_allowed":"true"}), true).is_err());
        assert!(!input_grant(&json!({"input_allowed":false}), true).unwrap());
        assert!(input_grant(&json!({"input_allowed":true}), true).unwrap());
    }
}
