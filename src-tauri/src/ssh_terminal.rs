//! Native SSH terminal adapter. The WebView receives the existing v3 cell records.
mod catalog;
mod reload;

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
    pending_snapshot: Option<u32>,
    focus_reporting: bool,
    mouse_reporting: bool,
    keypad_reporting: bool,
    keyboard_event_reporting: bool,
}

#[tauri::command]
pub(crate) async fn ssh_terminal<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
    mut action: Action,
    terminal: tauri::State<'_, Arc<Mutex<SshTerminal>>>,
    keyboard: tauri::State<'_, Arc<Mutex<super::native_keyboard::Samples>>>,
) -> Result<Value, String> {
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
        terminal.execute(action, store.as_ref(), &label)
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
    fn execute(
        &mut self,
        action: Action,
        store: Option<&Store>,
        window: &str,
    ) -> Result<Value, String> {
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
            return self.claim_reload(&ticket, window);
        }
        if matches!(action, Action::RecoverReloadLayout) {
            return self.recover_reload_layout(window);
        }
        if self.pending_reload.is_some() {
            return Err("The SSH writer is locked while its document is replaced.".into());
        }
        match action {
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
                };
                session.negotiate_keyboard_events();
                let keyboard_event_reporting = session.keyboard_event_reporting;
                self.session = Some(session);
                Ok(json!({
                    "owner": opened,
                    "save_warning": remembered.err(),
                    "keyboard_event_reporting": keyboard_event_reporting,
                }))
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
                        self.retained_reload = None;
                        if let Some(warning) = warning {
                            return Ok(json!({"ended":true,"message":warning}));
                        }
                        return Ok(Value::Null);
                    }
                    Err(reason) if reason == HOST_CHANGED => {
                        self.session = None;
                        self.retained_reload = None;
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
                    Command::EnableKeyboardEvents { .. } => {
                        return Err("The SSH desktop negotiates keyboard reporting on open.".into());
                    }
                    Command::Recover { owner } => return session.recover(owner.clone()),
                    Command::Detach { owner }
                    | Command::Poll { owner }
                    | Command::Ack { owner, .. }
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
                        self.retained_reload = None;
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
        if recovered.get("input_allowed") != Some(&Value::Bool(true)) {
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

    fn claim_reload(&mut self, ticket: &str, window: &str) -> Result<Value, String> {
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
                result
                    .as_object_mut()
                    .ok_or("Invalid SSH handoff snapshot.")?
                    .insert(
                        "view".into(),
                        serde_json::to_value(pending.view)
                            .map_err(|_| "Invalid retained layout.")?,
                    );
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
            focus_reporting: false,
            mouse_reporting: false,
            keypad_reporting: false,
            keyboard_event_reporting: false,
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

    fn negotiate_keyboard_events(&mut self) {
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
        self.focus_reporting = false;
        self.mouse_reporting = false;
        self.keypad_reporting = false;
        self.keyboard_event_reporting = false;
        self.pending_snapshot = Some(revision);
        Ok(())
    }

    fn recover(&mut self, previous: Attachment) -> Result<Value, String> {
        self.recover_with_grant(previous, false)
    }

    fn recover_with_grant(
        &mut self,
        previous: Attachment,
        explicit: bool,
    ) -> Result<Value, String> {
        if previous != self.owner && self.recovered_from.as_ref() != Some(&previous) {
            return Err("The SSH terminal attachment changed.".into());
        }
        let mut result = self.exchange(
            Command::Recover {
                owner: previous.clone(),
            },
            true,
        )?;
        let host_allowed = input_grant(&result, explicit)?;
        self.install_snapshot(&result, &previous)?;
        self.recovered_from = Some(previous);
        self.input_uncertain |= !host_allowed;
        let allowed = !self.read_only && !self.input_uncertain;
        self.keyboard_event_reporting = allowed
            && self.instance.as_deref().is_some_and(valid_identity)
            && result.get("keyboard_events_enabled") == Some(&Value::Bool(true));
        let receipt = result
            .as_object_mut()
            .ok_or("Invalid SSH terminal recovery result.")?;
        receipt.insert("input_allowed".into(), Value::Bool(allowed));
        receipt.insert(
            "keyboard_event_reporting".into(),
            Value::Bool(self.keyboard_event_reporting),
        );
        Ok(result)
    }

    fn exchange(&mut self, mut command: Command, reconnect: bool) -> Result<Value, String> {
        if self.relay.is_none() {
            self.focus_reporting = false;
            self.mouse_reporting = false;
            self.keypad_reporting = false;
            self.keyboard_event_reporting = false;
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

        if reconnect && self.relay.is_none() {
            self.focus_reporting = false;
            self.mouse_reporting = false;
            self.keypad_reporting = false;
            self.relay = Some(connect(&self.endpoint, self.instance.as_deref())?);
        }
        let relay = self
            .relay
            .as_mut()
            .ok_or("SSH terminal connection ended.")?;
        if !self.keypad_reporting && !strip_unavailable_keypad(&mut command) {
            return Ok(Value::Null);
        }
        let input =
            is_input(&command) && !matches!(command, Command::Focus { .. } | Command::Mouse { .. });
        let status = matches!(command, Command::Status { .. });
        let retiring = matches!(command, Command::Recover { .. } | Command::Detach { .. });
        if retiring {
            self.focus_reporting = false;
            self.mouse_reporting = false;
            self.keypad_reporting = false;
            self.keyboard_event_reporting = false;
        }
        match relay.call(command) {
            Ok(mut reply) => {
                if reply.is_err() {
                    self.focus_reporting = false;
                    self.mouse_reporting = false;
                    self.keypad_reporting = false;
                }
                if status {
                    self.focus_reporting = reply
                        .as_ref()
                        .ok()
                        .is_some_and(|value| focus_capability(self.instance.as_deref(), value));
                    self.mouse_reporting = reply
                        .as_ref()
                        .ok()
                        .is_some_and(|value| mouse_capability(self.instance.as_deref(), value));
                    self.keypad_reporting = reply
                        .as_ref()
                        .ok()
                        .is_some_and(|value| keypad_capability(self.instance.as_deref(), value));
                    if let Ok(value) = &mut reply {
                        if let Some(object) = value.as_object_mut() {
                            object.insert(
                                "focus_reporting".into(),
                                Value::Bool(self.focus_reporting),
                            );
                            object.insert(
                                "mouse_reporting".into(),
                                Value::Bool(self.mouse_reporting),
                            );
                            if !self.mouse_reporting {
                                object.remove("mouse");
                                object.remove("alternate_scroll");
                            }
                        }
                    }
                }
                reply
            }
            Err(_) => {
                self.relay = None;
                self.focus_reporting = false;
                self.mouse_reporting = false;
                self.keypad_reporting = false;
                self.keyboard_event_reporting = false;
                self.input_uncertain |= input;
                Err("SSH terminal connection ended.".into())
            }
        }
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
        session.pending_snapshot = Some(1);
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
