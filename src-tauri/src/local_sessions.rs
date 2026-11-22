//! Local desktop ownership, separate from the SSH transport.
mod detached_worker;
mod raw_suffix;
use detached_worker::DetachedWorker;
use raw_suffix::RawSuffix;
use std::{
    collections::{btree_map::Entry, BTreeMap},
    sync::{Mutex, Weak},
};
use tauri_terminal_frame_bridge::{
    live::{CloseStatus, LiveBridge},
    semantic::{Attachment, Resume, SessionSelector},
};

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
    raw: RawSuffix<Attachment>,
    target: SessionSelector,
    pump_candidate: bool,
}

#[derive(Default)]
pub(crate) struct LocalSessions {
    registry: Registry<OwnedLocalPty, 2>,
    last_epoch: u32,
    pump_after: Option<String>,
    detached_worker: Option<DetachedWorker>,
}

impl LocalSessions {
    pub(crate) fn open(&mut self) -> Result<Attachment, String> {
        // Keep the ordinary opener fail-closed if *any* PTY remains.
        if !self.registry.sessions.is_empty() {
            return Err("close the existing session first".into());
        }
        self.open_fresh()
    }

    pub(crate) fn open_second(&mut self, retained_epoch: u32) -> Result<Attachment, String> {
        if self.registry.sessions.len() != 1 || self.registry.reader.is_some() {
            return Err("detach the single existing local shell before opening a second".into());
        }
        self.target_for_epoch(retained_epoch)?;
        self.open_fresh()
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

    pub(crate) fn attach_selected(&mut self, epoch: u32) -> Result<Option<Resume>, String> {
        let target = self.target_for_epoch(epoch)?;
        self.attach(Some(&target))
    }

    fn open_fresh(&mut self) -> Result<Attachment, String> {
        let prev_epoch = self.last_epoch;
        let owner = self.registry.open(|| {
            let mut bridge = LiveBridge::local_desktop_after(prev_epoch);
            let epoch = bridge.open(24, 80)?;
            let owner = bridge.attachment(epoch)?;
            let owned = OwnedLocalPty {
                bridge,
                raw: RawSuffix::default(),
                target: SessionSelector {
                    session: owner.session.clone(),
                    epoch,
                },
                pump_candidate: false,
            };
            Ok((owner.session.clone(), owned, owner))
        })?;
        self.last_epoch = owner.epoch;
        Ok(owner)
    }

    pub(crate) fn attach(
        &mut self,
        target: Option<&SessionSelector>,
    ) -> Result<Option<Resume>, String> {
        self.registry
            .attach(target.map(|target| target.session.as_str()), |owned| {
                let view = owned
                    .bridge
                    .attach_read_only(target)?
                    .ok_or("terminal session closed")?;
                owned.raw.retain_only_current(&view.owner);
                owned.pump_candidate = false;
                Ok(view)
            })
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
            .validate_attachment(owner)?;
        if self.detached_worker.is_none() {
            self.detached_worker = Some(DetachedWorker::start(shared)?);
        }
        self.registry.detach(&owner.session, |owned| {
            owned.bridge.detach(owner)?;
            owned.raw.retire_view();
            owned.pump_candidate = matches!(owned.raw, RawSuffix::None);
            Ok(())
        })?;
        self.detached_worker.as_ref().unwrap().wake();
        Ok(())
    }

    fn pump_detached(&mut self) -> bool {
        let eligible =
            |owned: &OwnedLocalPty| owned.pump_candidate && matches!(owned.raw, RawSuffix::None);
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

    pub(crate) fn recover(&mut self, owner: &Attachment) -> Result<Resume, String> {
        // Keep LiveBridge's idempotent previous-generation recovery rules.
        let owned = self.registry.reader(&owner.session)?;
        let view = owned
            .bridge
            .resume(Some(owner))?
            .ok_or("terminal session closed")?;
        owned.raw.retain_only_current(&view.owner);
        Ok(view)
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
        let bridge = &mut self.registry.reader(&owner.session)?.bridge;
        let epoch = bridge.validate_input(owner)?;
        operation(bridge, epoch)
    }

    pub(crate) fn write_raw(&mut self, owner: Attachment, bytes: Vec<u8>) -> Result<usize, String> {
        let owned = self.registry.reader(&owner.session)?;
        let epoch = owned.bridge.validate_input(&owner)?;
        owned
            .raw
            .write(owner, bytes, |bytes| owned.bridge.write(epoch, bytes))
    }

    pub(crate) fn close(&mut self, owner: &Attachment) -> Result<CloseStatus, String> {
        // An already issued close still addresses this exact stable PTY and
        // epoch if recovery has since rotated its view generation.
        let (_, result) = self.registry.close(&owner.session, |owned| {
            let epoch = owned.bridge.validate_session(owner)?;
            owned.bridge.close(epoch)
        })?;
        if let Some(worker) = &self.detached_worker {
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
