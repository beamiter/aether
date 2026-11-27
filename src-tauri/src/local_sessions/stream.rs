//! A single delivery credit for the active local view. The PTY keeps draining
//! while the WebView validates, paints and acknowledges the current envelope.
use super::LocalShellStatus;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::ipc::Channel;
pub(crate) use tauri_terminal_frame_bridge::protocol::ObserverTarget;
pub(crate) use tauri_terminal_frame_bridge::protocol_v4::{Counter, PeerUpdate, Receipt};
use tauri_terminal_frame_bridge::{
    live::LiveBridge, protocol_v4, semantic::Attachment, SnapshotFormat,
};

pub(crate) type StreamEvent = protocol_v4::StreamEvent<LocalShellStatus>;

const HEARTBEAT: Duration = Duration::from_secs(2);
pub(super) const MAX_OBSERVERS: usize = 2;

type ObserverCursor = (ObserverTarget, Option<Counter>);
pub(super) type Preview = (ObserverTarget, Counter, PeerUpdate);

#[derive(Default)]
struct Observers {
    cursors: Vec<ObserverCursor>,
    next: usize,
}

impl Observers {
    fn set(&mut self, targets: &[ObserverTarget]) {
        if self.cursors.iter().map(|(target, _)| target).eq(targets) {
            return;
        }
        self.cursors = targets
            .iter()
            .map(|target| {
                let revision = self
                    .cursors
                    .iter()
                    .find(|(old, _)| old == target)
                    .and_then(|(_, revision)| *revision);
                (*target, revision)
            })
            .collect();
        self.next = 0;
    }

    fn candidates(&self) -> impl Iterator<Item = ObserverCursor> + '_ {
        let count = self.cursors.len();
        (0..count).map(move |offset| self.cursors[(self.next + offset) % count])
    }

    fn published(&mut self, target: ObserverTarget, revision: Counter) {
        if let Some((index, (_, cursor))) = self
            .cursors
            .iter_mut()
            .enumerate()
            .find(|(_, (old, _))| *old == target)
        {
            *cursor = Some(revision);
            self.next = (index + 1) % self.cursors.len();
        }
    }
}

pub(super) struct Delivery {
    sink: Channel<StreamEvent>,
    failed: Arc<AtomicBool>,
    event: StreamEvent,
}

impl Delivery {
    pub(super) fn send(self) {
        // Never invoke a WebView callback under the registry mutex.
        if self.sink.send(self.event).is_err() {
            self.failed.store(true, Ordering::Relaxed);
        }
    }
}

pub(super) struct LocalStream {
    pub(super) owner: Attachment,
    pub(super) format: SnapshotFormat,
    sink: Channel<StreamEvent>,
    failed: Arc<AtomicBool>,
    sequence: u64,
    pending: Option<Receipt>,
    acknowledged: Option<Receipt>,
    last_status: Option<LocalShellStatus>,
    last_sent: Instant,
    error: Option<String>,
    stopped: bool,
    observers: Observers,
}

impl LocalStream {
    pub(super) fn new(
        owner: Attachment,
        sink: Channel<StreamEvent>,
        format: SnapshotFormat,
    ) -> Self {
        Self {
            owner,
            format,
            sink,
            failed: Arc::new(AtomicBool::new(false)),
            sequence: 0,
            pending: None,
            acknowledged: None,
            last_status: None,
            last_sent: Instant::now(),
            error: None,
            stopped: false,
            observers: Observers::default(),
        }
    }

    pub(super) fn id(&self) -> u32 {
        self.sink.id()
    }

    pub(super) fn set_observers(&mut self, targets: &[ObserverTarget]) {
        self.observers.set(targets);
    }

    pub(super) fn observations(&self) -> impl Iterator<Item = ObserverCursor> + '_ {
        let can_send = !self.stopped
            && self.pending.is_none()
            && self.error.is_none()
            && !self.failed.load(Ordering::Relaxed);
        self.observers
            .candidates()
            .take(if can_send { MAX_OBSERVERS } else { 0 })
    }

    pub(super) fn acknowledge(
        &mut self,
        bridge: &mut LiveBridge,
        receipt: Receipt,
    ) -> Result<(), String> {
        if self.acknowledged == Some(receipt) {
            return Ok(());
        }
        if self.pending != Some(receipt) {
            return Err("wrong local stream acknowledgement".into());
        }
        if let Some(revision) = receipt.revision {
            bridge.acknowledge_publication(self.owner.epoch, revision.get())?;
        }
        self.pending = None;
        self.acknowledged = Some(receipt);
        Ok(())
    }

    pub(super) fn pump(
        &mut self,
        bridge: &mut LiveBridge,
        preview: Option<Preview>,
    ) -> (bool, Option<Delivery>) {
        let epoch = self.owner.epoch;
        if self.stopped || self.failed.load(Ordering::Relaxed) {
            return (bridge.pump_attached_io(epoch).unwrap_or(false), None);
        }
        if self.pending.is_some() {
            if self.error.is_some() {
                return (false, None);
            }
            return match bridge.pump_attached_io(epoch) {
                Ok(ready) => (ready, None),
                Err(error) => {
                    self.error = Some(error);
                    (false, None)
                }
            };
        }
        if let Some(error) = self.error.take() {
            return self.stop(error);
        }

        match self.poll(bridge, preview) {
            Ok(result) => result,
            Err(error) => self.stop(error),
        }
    }

    fn poll(
        &mut self,
        bridge: &mut LiveBridge,
        preview: Option<Preview>,
    ) -> Result<(bool, Option<Delivery>), String> {
        let frame = bridge.poll_channel_frame(self.owner.epoch, self.sequence == 0, self.format)?;
        // An initial publication can be deferred under encoder pressure. Do
        // not let a status-only update consume the sequence for the full base.
        if self.sequence == 0 && frame.is_none() {
            return Ok((true, None));
        }
        let status = LocalShellStatus::read(bridge, self.owner.epoch)?;
        let now = Instant::now();
        let ready = !status.drained();
        if frame.is_none()
            && preview.is_none()
            && self.last_status.as_ref() == Some(&status)
            && (!ready || now.duration_since(self.last_sent) < HEARTBEAT)
        {
            return Ok((ready, None));
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .and_then(Counter::new)
            .ok_or("local stream revision exhausted")?;
        self.sequence = sequence.get();
        self.pending = Some(Receipt {
            sequence,
            revision: frame.as_ref().map(|(revision, _)| *revision),
        });
        self.last_status = Some(status.clone());
        self.last_sent = now;
        if let Some((target, revision, _)) = &preview {
            self.observers.published(*target, *revision);
        }
        Ok((
            ready,
            Some(self.delivery(StreamEvent::Update {
                owner: self.owner.clone(),
                sequence,
                frame: frame.map(|(_, frame)| frame),
                preview: preview.map(|(_, _, frame)| Box::new(frame)),
                status,
            })),
        ))
    }

    fn stop(&mut self, message: String) -> (bool, Option<Delivery>) {
        self.stopped = true;
        (
            true,
            Some(self.delivery(StreamEvent::Stopped {
                owner: self.owner.clone(),
                message,
            })),
        )
    }

    fn delivery(&self, event: StreamEvent) -> Delivery {
        Delivery {
            sink: self.sink.clone(),
            failed: self.failed.clone(),
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_sessions::{LocalSessions, TabActivation};
    use serde_json::Value;
    use std::sync::{mpsc, Mutex};
    use tauri::ipc::InvokeResponseBody;

    #[test]
    fn observers_rotate_fairly_and_keep_independent_publication_revisions() {
        let first = ObserverTarget {
            epoch: 1,
            layout: 20,
        };
        let second = ObserverTarget {
            epoch: 2,
            layout: 21,
        };
        let new_surface = ObserverTarget {
            epoch: 2,
            layout: 22,
        };
        let high = Counter::new(999).unwrap();
        let low = Counter::new(3).unwrap();
        let order = |set: &Observers| {
            set.candidates()
                .map(|(target, revision)| (target.epoch, target.layout, revision))
                .collect::<Vec<_>>()
        };
        let mut set = Observers::default();
        assert!(order(&set).is_empty());
        set.set(&[first, second]);
        assert_eq!(order(&set), [(1, 20, None), (2, 21, None)]);
        set.published(first, high);
        assert_eq!(order(&set), [(2, 21, None), (1, 20, Some(high))]);
        set.published(second, low);
        assert_eq!(order(&set), [(1, 20, Some(high)), (2, 21, Some(low))]);
        set.published(first, high);
        set.set(&[first, second]);
        assert_eq!(order(&set)[0], (2, 21, Some(low)));
        set.set(&[new_surface, first]);
        assert_eq!(order(&set), [(2, 22, None), (1, 20, Some(high))]);
    }

    #[test]
    fn two_retained_ptys_share_one_delivery_credit_and_never_gain_input() {
        let local = Arc::new(Mutex::new(LocalSessions::default()));
        let shared = Arc::downgrade(&local);
        let mut sessions = local.lock().unwrap();
        let first = sessions
            .open(false, SnapshotFormat::Channel, None, shared.clone())
            .unwrap()
            .owner;
        let mut add_tab = |owner: &Attachment| {
            let attached = sessions
                .activate_tab(
                    Some(owner),
                    None,
                    TabActivation {
                        preserve_input: true,
                        integrated: false,
                        format: SnapshotFormat::Channel,
                        history: None,
                    },
                    shared.clone(),
                )
                .unwrap()
                .view;
            let tauri_terminal_frame_bridge::LocalSnapshot::Channel(frame) = attached.frame else {
                panic!("v4 tab activation did not return a snapshot");
            };
            let frame: protocol_v4::Update = serde_json::from_str(&frame).unwrap();
            sessions
                .acknowledge_frame(&attached.owner, frame.revision().get())
                .unwrap();
            attached.owner
        };
        let second = add_tab(&first);
        let active = add_tab(&second);
        let first_target = ObserverTarget {
            epoch: first.epoch,
            layout: 71,
        };
        let second_target = ObserverTarget {
            epoch: second.epoch,
            layout: 72,
        };
        let (send, receive) = mpsc::channel();
        let channel = Channel::new(move |body| {
            if let InvokeResponseBody::Json(json) = body {
                send.send(serde_json::from_str::<Value>(&json).unwrap())
                    .unwrap();
            }
            Ok(())
        });
        let id = channel.id();
        sessions
            .subscribe(
                active.clone(),
                channel,
                None,
                SnapshotFormat::Channel,
                shared,
            )
            .unwrap();
        sessions
            .set_observers(&active, id, &[first_target, second_target])
            .unwrap();
        assert!(sessions
            .set_observers(&active, id, &[first_target, first_target])
            .is_err());
        assert!(sessions
            .set_observers(&active, id, &[first_target, second_target, first_target])
            .is_err());
        assert!(sessions
            .set_observers(
                &active,
                id,
                &[ObserverTarget {
                    epoch: active.epoch,
                    layout: 73
                }]
            )
            .is_err());
        assert!(sessions
            .set_observers(&first, id, &[second_target])
            .is_err());
        assert!(sessions.input(&first, |_, _| Ok(())).is_err());
        assert!(sessions.input(&second, |_, _| Ok(())).is_err());
        assert!(sessions.input(&active, |_, _| Ok(())).is_ok());
        drop(sessions);

        let mut seen = Vec::new();
        for sequence in 1..=6 {
            let event = receive.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(event["owner"]["epoch"], active.epoch);
            assert_eq!(event["sequence"], sequence.to_string());
            if let Some(preview) = event["preview"].as_object() {
                let layout = preview["layout"].as_u64().unwrap();
                let frame: Value =
                    serde_json::from_str(preview["frame"].as_str().unwrap()).unwrap();
                let expected = if layout == 71 {
                    &first
                } else {
                    assert_eq!(layout, 72);
                    &second
                };
                assert_eq!(frame["stamp"]["epoch"], expected.epoch.to_string());
                assert_eq!(frame["stamp"]["session"], expected.session);
                if !seen.contains(&layout) {
                    seen.push(layout);
                }
            }
            assert!(
                receive.recv_timeout(Duration::from_millis(35)).is_err(),
                "second envelope before acknowledgement"
            );
            let revision = event["frame"].as_str().map(|raw| {
                serde_json::from_str::<protocol_v4::Update>(raw)
                    .unwrap()
                    .revision()
            });
            local
                .lock()
                .unwrap()
                .acknowledge_stream(&active, id, Counter::new(sequence).unwrap(), revision)
                .unwrap();
            if seen.len() == 2 {
                break;
            }
        }

        let mut sessions = local.lock().unwrap();
        for owner in [&first, &second, &active] {
            sessions.close(owner).unwrap();
        }
        assert_eq!(
            seen,
            [71, 72],
            "both retained PTYs must publish a preview in order"
        );
    }

    #[test]
    fn a_slow_channel_holds_one_issued_frame_while_a_real_pty_drains() {
        let local = Arc::new(Mutex::new(LocalSessions::default()));
        let owner = local
            .lock()
            .unwrap()
            .open(false, SnapshotFormat::Channel, None, Arc::downgrade(&local))
            .unwrap()
            .owner;
        let (send, receive) = mpsc::channel();
        let channel = Channel::new(move |body| {
            if let InvokeResponseBody::Json(json) = body {
                send.send(serde_json::from_str::<Value>(&json).unwrap())
                    .unwrap();
            }
            Ok(())
        });
        let channel_id = channel.id();
        local
            .lock()
            .unwrap()
            .subscribe(
                owner.clone(),
                channel,
                None,
                SnapshotFormat::Channel,
                Arc::downgrade(&local),
            )
            .unwrap();
        let first = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        let first_frame: Value = serde_json::from_str(first["frame"].as_str().unwrap()).unwrap();
        let first_revision: Counter =
            serde_json::from_value(first_frame["stamp"]["revision"].clone()).unwrap();
        let first_sequence = Counter::new(1).unwrap();
        assert_eq!(first["sequence"], "1");
        assert_eq!(first_frame["stamp"]["version"], 4);
        assert_eq!(first_frame["kind"], "snapshot");
        assert_eq!(first["owner"]["session"], owner.session);
        {
            let mut local = local.lock().unwrap();
            assert!(
                local.poll(&owner).is_err(),
                "poll cannot steal a subscribed frame"
            );
            assert!(local
                .acknowledge_frame(&owner, first_revision.get())
                .is_err());
            assert!(local
                .acknowledge_stream(
                    &owner,
                    channel_id.wrapping_add(1),
                    first_sequence,
                    Some(first_revision)
                )
                .is_err());
            assert!(local
                .acknowledge_stream(&owner, channel_id, first_sequence, None)
                .is_err());
        }

        // Much more than the Linux PTY buffer, so exit + EOF is evidence
        // the independent worker continued reading without a frame ACK.
        let command = b"stty -echo; head -c 262144 /dev/zero | tr '\\000' x; printf '\\r\\nAETHER_CHANNEL_TAIL\\r\\n'; exit\n";
        assert_eq!(
            local
                .lock()
                .unwrap()
                .write_raw(owner.clone(), command.to_vec())
                .unwrap(),
            command.len()
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = local
                .lock()
                .unwrap()
                .attached(&owner, |bridge, epoch| bridge.status(epoch))
                .unwrap();
            assert!(
                receive.try_recv().is_err(),
                "the channel exceeded its single delivery credit"
            );
            if status.drained {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the unacknowledged frame blocked PTY I/O"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        local
            .lock()
            .unwrap()
            .acknowledge_stream(&owner, channel_id, first_sequence, Some(first_revision))
            .unwrap();
        let final_event = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(final_event["sequence"], "2");
        assert_eq!(final_event["status"]["drained"], true);
        let final_frame: Value =
            serde_json::from_str(final_event["frame"].as_str().unwrap()).unwrap();
        let rows = final_frame["rows"].as_array().unwrap();
        assert!(rows.iter().any(|row| {
            let cells = row["row"]["cells"]
                .as_array()
                .or_else(|| row["cells"].as_array())
                .unwrap();
            cells
                .iter()
                .filter_map(|cell| cell["text"].as_str())
                .collect::<String>()
                .contains("AETHER_CHANNEL_TAIL")
        }));
        let mut local = local.lock().unwrap();
        let recovered = local
            .recover(&owner, tauri_terminal_frame_bridge::SnapshotFormat::Channel)
            .unwrap();
        let target = recovered.view.owner;
        assert_ne!(target.generation, owner.generation);
        assert!(local
            .acknowledge_stream(&owner, channel_id, Counter::new(2).unwrap(), None)
            .is_err());
        let tauri_terminal_frame_bridge::LocalSnapshot::Channel(frame) = recovered.view.frame
        else {
            panic!("expected v4 recovery snapshot");
        };
        let frame: tauri_terminal_frame_bridge::protocol_v4::Update =
            serde_json::from_str(&frame).unwrap();
        local
            .acknowledge_frame(&target, frame.revision().get())
            .unwrap();
        local.close(&target).unwrap();
    }
}
