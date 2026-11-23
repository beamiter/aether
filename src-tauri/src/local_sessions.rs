//! Local desktop ownership, separate from the SSH transport.
mod chat;
mod stream;
mod worker;
pub(crate) use chat::LocalShellStatus;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    sync::{Mutex, Weak},
};
pub(crate) use stream::{Counter, ObserverTarget, StreamEvent};
use stream::{Delivery, LocalStream, PeerUpdate, Receipt};
use tauri::ipc::Channel;
use tauri_terminal_frame_bridge::{
    live::{CloseStatus, LiveBridge},
    semantic::{Attachment, Resume, SessionSelector, Update},
    snapshot_encoder, LocalSnapshot, SnapshotFormat, WireFrame,
};
use worker::LocalWorker;

/// Stable PTY ownership and the single reader lease. View generations and
/// input permissions remain inside each bridge, not in the table key.
struct Registry<T, const LIMIT: usize> {
    sessions: BTreeMap<String, T>,
    reader: Option<String>,
}

impl<T, const LIMIT: usize> Default for Registry<T, LIMIT> {
    fn default() -> Self {
        Self {
            sessions: BTreeMap::new(),
            reader: None,
        }
    }
}

impl<T, const LIMIT: usize> Registry<T, LIMIT> {
    fn open<R>(
        &mut self,
        create: impl FnOnce() -> Result<(String, T, R), String>,
    ) -> Result<R, String> {
        if self.reader.is_some() || self.sessions.len() >= LIMIT {
            return Err("close the existing session first".into());
        }
        let (id, value, reply) = create()?;
        match self.sessions.entry(id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(value);
                self.reader = Some(id);
                Ok(reply)
            }
            Entry::Occupied(_) => Err("terminal session ID collision".into()),
        }
    }

    fn owned(&mut self, id: &str) -> Result<&mut T, String> {
        self.sessions
            .get_mut(id)
            .ok_or_else(|| "foreign terminal session".into())
    }

    fn reader(&mut self, id: &str) -> Result<&mut T, String> {
        if self.reader.as_deref() != Some(id) {
            return Err("stale terminal attachment".into());
        }
        self.owned(id)
    }

    fn detach(
        &mut self,
        id: &str,
        operation: impl FnOnce(&mut T) -> Result<(), String>,
    ) -> Result<(), String> {
        operation(self.reader(id)?)?;
        self.reader = None;
        Ok(())
    }

    fn attach<R>(
        &mut self,
        id: Option<&str>,
        operation: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<Option<R>, String> {
        let id = match id {
            Some(id) => id.to_owned(),
            None if self.sessions.is_empty() => return Ok(None),
            None if self.sessions.len() == 1 => self.sessions.keys().next().unwrap().clone(),
            None => return Err("select the retained terminal session".into()),
        };
        if self.reader.as_deref().is_some_and(|reader| reader != id) {
            return Err("detach the current terminal view first".into());
        }
        let result = operation(self.owned(&id)?)?;
        self.reader = Some(id);
        Ok(Some(result))
    }

    fn next_inactive(
        &mut self,
        after: Option<&str>,
        ready: impl Fn(&T) -> bool,
    ) -> Option<(&String, &mut T)> {
        let active = self.reader.as_deref();
        let admitted = |id: &String, value: &T| active != Some(id.as_str()) && ready(value);
        // One bounded eligible turn; wrap in stable key order. The desktop
        // cap is two, so even the skipped/blocked scan is bounded.
        let mut fallback = None;
        for (id, value) in &mut self.sessions {
            if !admitted(id, value) {
                continue;
            }
            if after.is_none_or(|after| id.as_str() > after) {
                return Some((id, value));
            }
            if fallback.is_none() {
                fallback = Some((id, value));
            }
        }
        fallback
    }

    fn close<R>(
        &mut self,
        id: &str,
        operation: impl FnOnce(&mut T) -> Result<R, String>,
    ) -> Result<(T, R), String> {
        let result = operation(self.owned(id)?)?;
        let removed = self.sessions.remove(id).expect("validated terminal owner");
        if self.reader.as_deref() == Some(id) {
            self.reader = None;
        }
        Ok((removed, result))
    }
}

struct OwnedLocalPty {
    bridge: LiveBridge,
    target: SessionSelector,
    pump_candidate: bool,
    parked_input: bool,
}

#[derive(serde::Serialize)]
pub(crate) struct AttachedView {
    #[serde(flatten)]
    view: Resume<LocalSnapshot>,
    input_allowed: bool,
}

struct PendingTab {
    source: Option<Attachment>,
    requested_epoch: Option<u32>,
    target: Attachment,
    input_allowed: bool,
}

#[derive(Default)]
pub(crate) struct LocalSessions {
    registry: Registry<OwnedLocalPty, 2>,
    last_epoch: u32,
    pump_after: Option<String>,
    worker: Option<LocalWorker>,
    pending_tab: Option<PendingTab>,
    stream: Option<LocalStream>,
}

impl LocalSessions {
    pub(crate) fn open(
        &mut self,
        integrated: bool,
        shared: Weak<Mutex<Self>>,
    ) -> Result<Attachment, String> {
        // Keep the ordinary opener fail-closed if *any* PTY remains.
        if !self.registry.sessions.is_empty() {
            return Err("close the existing session first".into());
        }
        self.open_fresh(integrated, shared)
    }

    pub(crate) fn open_second(
        &mut self,
        retained_epoch: u32,
        shared: Weak<Mutex<Self>>,
    ) -> Result<Attachment, String> {
        if self.registry.sessions.len() != 1 || self.registry.reader.is_some() {
            return Err("detach the single existing local shell before opening a second".into());
        }
        self.target_for_epoch(retained_epoch)?;
        self.open_fresh(false, shared)
    }

    pub(crate) fn list_epochs(&self) -> Vec<u32> {
        // Attachment IDs are also needed for close/input. Discovery exposes
        // only local epoch labels and cannot manufacture an attachment.
        let mut owned: Vec<_> = self
            .registry
            .sessions
            .values()
            .map(|pty| pty.target.epoch)
            .collect();
        owned.sort_unstable();
        owned
    }

    fn target_for_epoch(&self, epoch: u32) -> Result<SessionSelector, String> {
        self.registry
            .sessions
            .values()
            .find(|owned| owned.target.epoch == epoch)
            .map(|owned| owned.target.clone())
            .ok_or_else(|| "no such retained local shell".into())
    }

    pub(crate) fn attach_selected(
        &mut self,
        epoch: u32,
        format: SnapshotFormat,
    ) -> Result<Option<Resume<LocalSnapshot>>, String> {
        let target = self.target_for_epoch(epoch)?;
        self.attach(Some(&target), format)
    }

    fn open_fresh(
        &mut self,
        integrated: bool,
        shared: Weak<Mutex<Self>>,
    ) -> Result<Attachment, String> {
        self.ensure_worker(shared)?;
        let worker = self.worker.as_ref().unwrap();
        let prev_epoch = self.last_epoch;
        let owner = self.registry.open(|| {
            let (owned, owner) = Self::spawn(prev_epoch, integrated, worker)?;
            Ok((owner.session.clone(), owned, owner))
        })?;
        self.last_epoch = owner.epoch;
        self.worker.as_ref().unwrap().wake();
        Ok(owner)
    }

    fn spawn(
        prev_epoch: u32,
        integrated: bool,
        worker: &LocalWorker,
    ) -> Result<(OwnedLocalPty, Attachment), String> {
        let mut bridge = LiveBridge::local_desktop_after(prev_epoch);
        bridge.set_hyperlink_policy(jterm_core::link::is_openable_osc8);
        let epoch = if integrated {
            bridge.open_integrated(24, 80)?
        } else {
            bridge.open(24, 80)?
        };
        worker.watch(&bridge)?;
        let owner = bridge.attachment(epoch)?;
        let owned = OwnedLocalPty {
            bridge,
            target: SessionSelector {
                session: owner.session.clone(),
                epoch,
            },
            pump_candidate: false,
            parked_input: false,
        };
        Ok((owned, owner))
    }

    pub(crate) fn activate_tab(
        &mut self,
        owner: Option<&Attachment>,
        target_epoch: Option<u32>,
        preserve_input: bool,
        format: SnapshotFormat,
        shared: Weak<Mutex<Self>>,
    ) -> Result<AttachedView, String> {
        if let Some(pending) = self.pending_tab.as_ref().filter(|pending| {
            pending.source.as_ref() == owner && pending.requested_epoch == target_epoch
        }) {
            let owned = self.registry.reader(&pending.target.session)?;
            return Ok(AttachedView {
                view: Resume {
                    owner: pending.target.clone(),
                    frame: owned
                        .bridge
                        .pending_local_tab_with(&pending.target, snapshot_encoder(format))?,
                },
                input_allowed: pending.input_allowed,
            });
        }

        let preserve_input = if let Some(owner) = owner {
            let source = self.registry.reader(&owner.session)?;
            source.bridge.next_detached_generation(owner)?;
            if preserve_input && !source.bridge.can_park_local_input(owner) {
                return Err("Wait for the current shell input before switching tabs.".into());
            }
            preserve_input
        } else if self.registry.reader.is_some() {
            return Err("a local shell tab is already active".into());
        } else {
            false
        };
        let target = match target_epoch {
            Some(epoch) if !owner.is_some_and(|owner| owner.epoch == epoch) => {
                Some(self.target_for_epoch(epoch)?)
            }
            Some(_) => return Err("local tab already active".into()),
            None if owner.is_none() => return Err("select an existing shell tab".into()),
            None if self.registry.sessions.len() < 2 => None,
            None => return Err("only two local shell tabs can be open".into()),
        };
        self.ensure_worker(shared)?;

        let (view, input_allowed) = if let Some(target) = target {
            let owned = self.registry.owned(&target.session)?;
            let grant_input = owned.parked_input;
            let activated = owned.bridge.activate_local_tab_with(
                &target,
                grant_input,
                snapshot_encoder(format),
            )?;
            owned.pump_candidate = false;
            owned.parked_input = false;
            activated
        } else {
            let (mut owned, original) =
                Self::spawn(self.last_epoch, false, self.worker.as_ref().unwrap())?;
            let Entry::Vacant(entry) = self.registry.sessions.entry(original.session) else {
                return Err("terminal session ID collision".into());
            };
            let activated = owned.bridge.activate_local_tab_with(
                &owned.target,
                true,
                snapshot_encoder(format),
            )?;
            entry.insert(owned);
            self.last_epoch = original.epoch;
            activated
        };

        // This runs under the same registry mutex as preflight and target
        // publication. The validated source could not have changed in between.
        if let Some(owner) = owner {
            self.retire_view(owner, preserve_input)?;
        }
        let target = &view.owner;
        self.registry.reader = Some(target.session.clone());
        self.pending_tab = Some(PendingTab {
            source: owner.cloned(),
            requested_epoch: target_epoch,
            target: target.clone(),
            input_allowed,
        });
        self.worker.as_ref().unwrap().wake();
        Ok(AttachedView {
            view,
            input_allowed,
        })
    }

    pub(crate) fn attach(
        &mut self,
        target: Option<&SessionSelector>,
        format: SnapshotFormat,
    ) -> Result<Option<Resume<LocalSnapshot>>, String> {
        let attached =
            self.registry
                .attach(target.map(|target| target.session.as_str()), |owned| {
                    let view = owned
                        .bridge
                        .attach_read_only_with(target, snapshot_encoder(format))?
                        .ok_or("terminal session closed")?;
                    owned.pump_candidate = false;
                    owned.parked_input = false;
                    Ok(view)
                })?;
        if let Some(view) = &attached {
            self.retire_stale_stream(&view.owner);
            self.worker.as_ref().unwrap().wake();
        }
        Ok(attached)
    }

    pub(crate) fn detach(
        &mut self,
        owner: &Attachment,
        shared: Weak<Mutex<Self>>,
    ) -> Result<(), String> {
        // Thread creation is fallible. Do it only for an actual owner and
        // before detach changes the lease, so the caller can retry safely.
        self.registry
            .reader(&owner.session)?
            .bridge
            .next_detached_generation(owner)?;
        self.ensure_worker(shared)?;
        self.retire_view(owner, false)
    }

    fn ensure_worker(&mut self, shared: Weak<Mutex<Self>>) -> Result<(), String> {
        if self.worker.is_none() {
            self.worker = Some(LocalWorker::start(shared)?);
        }
        Ok(())
    }

    /// The native WebView started a new document. Retire its old attachment
    /// and any unknown input while leaving the already launched PTY running.
    pub(crate) fn retire_renderer(&mut self) -> Result<(), String> {
        if let Some(id) = self.registry.reader.clone() {
            let owned = self.registry.reader(&id)?;
            let owner = owned.bridge.attachment(owned.target.epoch)?;
            self.retire_view(&owner, false)?;
        }
        for session in self.registry.sessions.values_mut() {
            session.parked_input = false;
        }
        self.pending_tab = None;
        Ok(())
    }

    fn retire_view(&mut self, owner: &Attachment, parked_input: bool) -> Result<(), String> {
        self.registry.detach(&owner.session, |owned| {
            owned.bridge.detach(owner)?;
            owned.pump_candidate = true;
            owned.parked_input = parked_input;
            Ok(())
        })?;
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.owner == *owner)
        {
            self.stream = None;
        }
        self.worker.as_ref().unwrap().wake();
        Ok(())
    }

    fn pump_detached(&mut self) -> bool {
        let eligible = |owned: &OwnedLocalPty| owned.pump_candidate;
        let Some((id, owned)) = self
            .registry
            .next_inactive(self.pump_after.as_deref(), eligible)
        else {
            return false;
        };
        if self.pump_after.as_deref() != Some(id) {
            self.pump_after = Some(id.clone());
        }
        owned.pump_candidate = owned.bridge.pump_detached_idle().unwrap_or(false);
        // If the selected owner stopped, another inactive PTY may still be
        // admitted. It will get the next tick, never a second read this tick.
        owned.pump_candidate
            || self
                .registry
                .next_inactive(self.pump_after.as_deref(), eligible)
                .is_some()
    }

    fn retire_stale_stream(&mut self, current: &Attachment) {
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.owner != *current)
        {
            self.stream = None;
        }
    }

    pub(crate) fn subscribe(
        &mut self,
        owner: Attachment,
        sink: Channel<StreamEvent>,
        observer: Option<ObserverTarget>,
        shared: Weak<Mutex<Self>>,
    ) -> Result<(), String> {
        let bridge = &self.registry.reader(&owner.session)?.bridge;
        bridge.validate_attachment(&owner)?;
        if self.stream.is_some() || bridge.has_unacknowledged_frame() {
            return Err("recover the local view before subscribing again".into());
        }
        self.validate_observer(&owner, observer)?;
        self.ensure_worker(shared)?;
        let mut stream = LocalStream::new(owner, sink);
        stream.set_observer(observer);
        self.stream = Some(stream);
        self.worker.as_ref().unwrap().wake();
        Ok(())
    }

    fn pump(&mut self) -> (bool, Option<Delivery>) {
        let detached = self.pump_detached();
        let preview = self
            .stream
            .as_ref()
            .and_then(LocalStream::observe_next)
            .and_then(|(target, revision)| {
                self.retained_bridge(target.epoch)
                    .and_then(|bridge| bridge.observe_channel_frame(target.epoch, revision))
                    .ok()
                    .flatten()
                    .map(|(revision, frame)| {
                        (
                            revision,
                            PeerUpdate {
                                layout: target.layout,
                                frame,
                            },
                        )
                    })
            });
        let Some(stream) = self.stream.as_mut() else {
            let active = self
                .registry
                .reader
                .as_ref()
                .and_then(|id| self.registry.sessions.get_mut(id))
                .is_some_and(|owned| owned.bridge.pump_unpublished().unwrap_or(false));
            return (active || detached, None);
        };
        let Ok(owned) = self.registry.reader(&stream.owner.session) else {
            self.stream = None;
            return (detached, None);
        };
        if owned.bridge.validate_attachment(&stream.owner).is_err() {
            self.stream = None;
            return (detached, None);
        }
        let (active, delivery) = stream.pump(&mut owned.bridge, preview);
        (active || detached, delivery)
    }

    fn validate_observer(
        &self,
        owner: &Attachment,
        observer: Option<ObserverTarget>,
    ) -> Result<(), String> {
        if let Some(target) = observer {
            if target.epoch == owner.epoch || target.layout == 0 {
                return Err("invalid split pane observation".into());
            }
            self.target_for_epoch(target.epoch)?;
        }
        Ok(())
    }

    pub(crate) fn set_observer(
        &mut self,
        owner: &Attachment,
        stream_id: u32,
        target: Option<ObserverTarget>,
    ) -> Result<(), String> {
        self.registry
            .reader(&owner.session)?
            .bridge
            .validate_attachment(owner)?;
        self.validate_observer(owner, target)?;
        let stream = self
            .stream
            .as_mut()
            .filter(|stream| stream.owner == *owner && stream.id() == stream_id)
            .ok_or("stale local stream")?;
        stream.set_observer(target);
        self.worker.as_ref().unwrap().wake();
        Ok(())
    }

    pub(crate) fn observe_shell(
        &mut self,
        owner: &Attachment,
        epoch: u32,
        revision: Option<u32>,
    ) -> Result<Option<WireFrame>, String> {
        self.registry
            .reader(&owner.session)?
            .bridge
            .validate_attachment(owner)?;
        if self.stream.is_some() || epoch == owner.epoch {
            return Err("invalid split pane observation".into());
        }
        self.observe_snapshot(epoch, revision)
    }

    fn observe_snapshot(
        &self,
        epoch: u32,
        revision: Option<u32>,
    ) -> Result<Option<WireFrame>, String> {
        self.retained_bridge(epoch)?.observe_frame(epoch, revision)
    }

    fn retained_bridge(&self, epoch: u32) -> Result<&LiveBridge, String> {
        self.registry
            .sessions
            .values()
            .find(|owned| owned.target.epoch == epoch)
            .map(|owned| &owned.bridge)
            .ok_or_else(|| "no such retained local shell".into())
    }

    pub(crate) fn acknowledge_stream(
        &mut self,
        owner: &Attachment,
        stream_id: u32,
        sequence: Counter,
        revision: Option<Counter>,
    ) -> Result<(), String> {
        let owned = self.registry.reader(&owner.session)?;
        owned.bridge.validate_attachment(owner)?;
        let stream = self
            .stream
            .as_mut()
            .filter(|stream| stream.owner == *owner && stream.id() == stream_id)
            .ok_or("stale local stream")?;
        stream.acknowledge(&mut owned.bridge, Receipt { sequence, revision })?;
        self.worker.as_ref().unwrap().wake();
        Ok(())
    }

    pub(crate) fn poll(&mut self, owner: &Attachment) -> Result<Option<Update>, String> {
        if self.stream.is_some() {
            return Err("local channel owns frame publication".into());
        }
        self.attached(owner, LiveBridge::poll_update)
    }

    pub(crate) fn acknowledge_frame(
        &mut self,
        owner: &Attachment,
        revision: u64,
    ) -> Result<(), String> {
        if self.stream.is_some() {
            return Err("local channel owns frame publication".into());
        }
        self.attached(owner, |bridge, epoch| {
            bridge.acknowledge_publication(epoch, revision)
        })
    }

    pub(crate) fn recover(
        &mut self,
        owner: &Attachment,
        format: SnapshotFormat,
    ) -> Result<AttachedView, String> {
        // Keep LiveBridge's idempotent previous-generation recovery rules.
        let owned = self.registry.reader(&owner.session)?;
        let view = owned
            .bridge
            .resume_with(Some(owner), snapshot_encoder(format))?
            .ok_or("terminal session closed")?;
        let input_allowed = owned.bridge.can_resume_local_input();
        self.retire_stale_stream(&view.owner);
        Ok(AttachedView {
            view,
            input_allowed,
        })
    }

    pub(crate) fn attached<R>(
        &mut self,
        owner: &Attachment,
        operation: impl FnOnce(&mut LiveBridge, u32) -> Result<R, String>,
    ) -> Result<R, String> {
        let bridge = &mut self.registry.reader(&owner.session)?.bridge;
        let epoch = bridge.validate_attachment(owner)?;
        operation(bridge, epoch)
    }

    pub(crate) fn input<R>(
        &mut self,
        owner: &Attachment,
        operation: impl FnOnce(&mut LiveBridge, u32) -> Result<R, String>,
    ) -> Result<R, String> {
        let owned = self.registry.reader(&owner.session)?;
        let epoch = owned.bridge.validate_logical_input(owner)?;
        let result = operation(&mut owned.bridge, epoch);
        if let Some(worker) = &self.worker {
            worker.wake();
        }
        result
    }

    pub(crate) fn write_raw(&mut self, owner: Attachment, bytes: Vec<u8>) -> Result<usize, String> {
        let owned = self.registry.reader(&owner.session)?;
        owned.bridge.write_owned_raw(owner, bytes)
    }

    pub(crate) fn close(&mut self, owner: &Attachment) -> Result<CloseStatus, String> {
        // An already issued close still addresses this exact stable PTY and
        // epoch if recovery has since rotated its view generation.
        let (_, result) = self.registry.close(&owner.session, |owned| {
            let epoch = owned.bridge.validate_session(owner)?;
            owned.bridge.close(epoch)
        })?;
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.owner.session == owner.session)
        {
            self.stream = None;
        }
        if let Some(worker) = &self.worker {
            worker.wake();
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::Registry;

    #[test]
    fn two_retained_owners_have_only_one_reader_and_implicit_attach_is_ambiguous() {
        let mut table = Registry::<u8, 2>::default();
        table.open(|| Ok(("a".into(), 11, ()))).unwrap();
        table.detach("a", |_| Ok(())).unwrap();
        table.open(|| Ok(("b".into(), 22, ()))).unwrap();
        assert!(table.reader("a").is_err());
        assert_eq!(table.reader("b"), Ok(&mut 22));
        assert!(table
            .attach::<()>(Some("a"), |_| panic!("must not run"))
            .is_err());
        assert!(table
            .attach::<()>(None, |_| panic!("must not infer"))
            .is_err());
        table.detach("b", |_| Ok(())).unwrap();
        assert_eq!(table.attach(Some("a"), |v| Ok(*v)), Ok(Some(11)));
        assert!(table.reader("b").is_err());
        assert_eq!(table.close("b", |v| Ok(*v)), Ok((22, 22)));
        assert_eq!(table.reader("a"), Ok(&mut 11));
    }

    #[test]
    fn unsuccessful_transitions_leave_reader_and_owned_entry_unchanged() {
        let mut table = Registry::<u8, 1>::default();
        table.open(|| Ok(("a".into(), 11, ()))).unwrap();
        assert!(table
            .open::<()>(|| panic!("must not spawn at capacity"))
            .is_err());
        assert!(table
            .detach("a", |_| Err("stale generation".into()))
            .is_err());
        assert!(table.close::<()>("a", |_| Err("old epoch".into())).is_err());
        assert_eq!(table.reader("a"), Ok(&mut 11));
        table.detach("a", |_| Ok(())).unwrap();
        assert!(table
            .attach::<()>(Some("a"), |_| Err("wrong epoch".into()))
            .is_err());
        assert!(table.reader("a").is_err());
        assert_eq!(table.attach(None, |v| Ok(*v)), Ok(Some(11)));
        assert_eq!(table.close("a", |v| Ok(*v)), Ok((11, 11)));
        table.open(|| Ok(("b".into(), 22, ()))).unwrap();
        assert!(table.close::<()>("a", |_| panic!("wrong PTY")).is_err());
        assert_eq!(table.reader("b"), Ok(&mut 22));
    }
}
