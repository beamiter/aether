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
    observer: Option<ObserverTarget>,
    observer_revision: Option<Counter>,
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
            observer: None,
            observer_revision: None,
        }
    }

    pub(super) fn id(&self) -> u32 {
        self.sink.id()
    }

    pub(super) fn set_observer(&mut self, target: Option<ObserverTarget>) {
        if self.observer != target {
            self.observer = target;
            self.observer_revision = None;
        }
    }

    pub(super) fn observe_next(&self) -> Option<(ObserverTarget, Option<Counter>)> {
        if self.stopped
            || self.pending.is_some()
            || self.error.is_some()
            || self.failed.load(Ordering::Relaxed)
        {
            return None;
        }
        self.observer.map(|target| (target, self.observer_revision))
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
        preview: Option<(Counter, PeerUpdate)>,
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
        preview: Option<(Counter, PeerUpdate)>,
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
        if let Some((revision, _)) = &preview {
            self.observer_revision = Some(*revision);
        }
        Ok((
            ready,
            Some(self.delivery(StreamEvent::Update {
                owner: self.owner.clone(),
                sequence,
                frame: frame.map(|(_, frame)| frame),
                preview: preview.map(|(_, frame)| Box::new(frame)),
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
    use crate::local_sessions::LocalSessions;
    use serde_json::Value;
    use std::sync::{mpsc, Mutex};
    use tauri::ipc::InvokeResponseBody;

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
