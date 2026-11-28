//! A single delivery credit for the active local view. The PTY keeps draining
//! while the WebView validates, paints and acknowledges the current envelope.
use super::LocalShellStatus;
use serde::Serialize;
use std::{
    fmt::Write as _,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tauri::ipc::{Channel, InvokeResponseBody, IpcResponse};
pub(crate) use tauri_terminal_frame_bridge::protocol::ObserverTarget;
pub(crate) use tauri_terminal_frame_bridge::protocol_v4::{Counter, Receipt};
use tauri_terminal_frame_bridge::{
    live::LiveBridge, protocol_v4, semantic::Attachment, EncodedChannelFrame, LiveFrameFormat,
};

type QuotedEvent = protocol_v4::StreamEvent<LocalShellStatus>;
pub(crate) struct StreamEvent(StreamBody);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameDelivery {
    Quoted,
    Structured,
    BinaryRasters,
}

impl FrameDelivery {
    pub(super) fn binary_rasters(self) -> bool {
        self == Self::BinaryRasters
    }
}

enum StreamBody {
    Quoted(QuotedEvent),
    Direct(DirectUpdate),
}

struct DirectUpdate {
    owner: Attachment,
    sequence: Counter,
    frame: Option<EncodedChannelFrame>,
    preview: Option<(u32, EncodedChannelFrame)>,
    status: LocalShellStatus,
}

#[derive(Serialize)]
struct DirectHeader<'a> {
    kind: &'static str,
    owner: &'a Attachment,
    sequence: Counter,
    status: &'a LocalShellStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_bytes: Option<usize>,
}

impl DirectUpdate {
    fn json(&self) -> serde_json::Result<String> {
        let active = self
            .frame
            .as_ref()
            .map_or("null", EncodedChannelFrame::as_json);
        let mut json = serde_json::to_string(&DirectHeader {
            kind: "update",
            owner: &self.owner,
            sequence: self.sequence,
            status: &self.status,
            frame_bytes: self.frame.as_ref().map(|frame| frame.as_json().len()),
            image_bytes: self
                .frame
                .as_ref()
                .map(EncodedChannelFrame::image_bytes)
                .filter(|bytes| *bytes != 0),
        })?;
        json.reserve_exact(
            active.len()
                + self
                    .preview
                    .as_ref()
                    .map_or(0, |(_, frame)| frame.as_json().len())
                + 128,
        );
        // The header is a serialized object. Append only JSON from the
        // bounded frame encoder and numeric preview metadata.
        json.pop();
        json.push_str(",\"frame\":");
        json.push_str(active);
        if let Some((layout, frame)) = &self.preview {
            write!(
                json,
                ",\"preview\":{{\"layout\":{layout},\"frame_bytes\":{}",
                frame.as_json().len()
            )
            .expect("writing numbers to a String succeeds");
            if frame.image_bytes() != 0 {
                write!(json, ",\"image_bytes\":{}", frame.image_bytes())
                    .expect("writing numbers to a String succeeds");
            }
            json.push_str(",\"frame\":");
            json.push_str(frame.as_json());
            json.push('}');
        }
        json.push('}');
        Ok(json)
    }

    fn into_body(self) -> tauri::Result<InvokeResponseBody> {
        let json = self.json()?;
        let mut active = self
            .frame
            .map(EncodedChannelFrame::into_image_bytes)
            .unwrap_or_default();
        let peer = self
            .preview
            .map(|(_, frame)| frame.into_image_bytes())
            .unwrap_or_default();
        let mut bytes = if active.is_empty() {
            peer
        } else {
            active.extend_from_slice(&peer);
            active
        };
        if bytes.is_empty() {
            return Ok(InvokeResponseBody::Json(json));
        }
        let metadata_len = u32::try_from(json.len())
            .map_err(|_| std::io::Error::other("binary Channel metadata too large"))?;
        // Active then passive RGBA8, structured event JSON, and a fixed
        // trailer containing the metadata length and format version.
        bytes.extend_from_slice(json.as_bytes());
        bytes.extend_from_slice(&metadata_len.to_le_bytes());
        bytes.extend_from_slice(b"JTR1");
        Ok(InvokeResponseBody::Raw(bytes))
    }
}

impl IpcResponse for StreamEvent {
    fn body(self) -> tauri::Result<InvokeResponseBody> {
        match self.0 {
            StreamBody::Quoted(event) => event.body(),
            StreamBody::Direct(update) => update.into_body(),
        }
    }
}

const HEARTBEAT: Duration = Duration::from_secs(2);
pub(super) const MAX_OBSERVERS: usize = 2;

type ObserverCursor = (ObserverTarget, Option<Counter>);
pub(super) type Preview = (ObserverTarget, Counter, EncodedChannelFrame);

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
    pub(super) format: LiveFrameFormat,
    pub(super) frame_delivery: FrameDelivery,
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
        format: LiveFrameFormat,
        frame_delivery: FrameDelivery,
    ) -> Self {
        Self {
            owner,
            format,
            frame_delivery,
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
        let frame = bridge.poll_encoded_channel_frame(
            self.owner.epoch,
            self.sequence == 0,
            self.format,
            self.frame_delivery.binary_rasters(),
        )?;
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
        let owner = self.owner.clone();
        let frame = frame.map(|(_, frame)| frame);
        let preview = preview.map(|(target, _, frame)| (target.layout, frame));
        let body = if self.frame_delivery != FrameDelivery::Quoted {
            StreamBody::Direct(DirectUpdate {
                owner,
                sequence,
                frame,
                preview,
                status,
            })
        } else {
            StreamBody::Quoted(QuotedEvent::Update {
                owner,
                sequence,
                frame: frame.map(EncodedChannelFrame::into_json),
                preview: preview.map(|(layout, frame)| {
                    Box::new(protocol_v4::PeerUpdate {
                        layout,
                        frame: frame.into_json(),
                    })
                }),
                status,
            })
        };
        Ok((ready, Some(self.delivery(StreamEvent(body)))))
    }

    fn stop(&mut self, message: String) -> (bool, Option<Delivery>) {
        self.stopped = true;
        (
            true,
            Some(
                self.delivery(StreamEvent(StreamBody::Quoted(QuotedEvent::Stopped {
                    owner: self.owner.clone(),
                    message,
                }))),
            ),
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
    use tauri_terminal_frame_bridge::SnapshotFormat;

    #[test]
    fn binary_delivery_places_active_pixels_before_preview_pixels() {
        fn image_frame(rgb: &str) -> (Attachment, EncodedChannelFrame, LocalShellStatus) {
            let mut bridge = LiveBridge::local_desktop();
            bridge.enable_inline_graphics(LiveFrameFormat::Dense);
            let epoch = bridge.open(8, 32).unwrap();
            let owner = bridge.attachment(epoch).unwrap();
            let mut command = format!(
                r"printf '\033[2J\033[H\033_Ga=T,i=3,f=24,s=1,v=1,c=1,r=1,C=1;{rgb}\033\\'; read -r _"
            );
            command.push('\n');
            bridge
                .write_owned_raw(owner.clone(), command.into_bytes())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                bridge.pump_attached_io(epoch).unwrap();
                if let Some((_, frame)) = bridge
                    .observe_encoded_channel_frame(epoch, None, LiveFrameFormat::Dense, true)
                    .unwrap()
                {
                    if frame.image_bytes() == 4 {
                        let status = LocalShellStatus::read(&bridge, epoch).unwrap();
                        bridge.close(epoch).unwrap();
                        return (owner, frame, status);
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "inline image never reached the Channel"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        let (owner, active, status) = image_frame("/wAA");
        let (_, preview, _) = image_frame("AAD/");
        let body = DirectUpdate {
            owner,
            sequence: Counter::new(1).unwrap(),
            frame: Some(active),
            preview: Some((7, preview)),
            status,
        }
        .into_body()
        .unwrap();
        let InvokeResponseBody::Raw(body) = body else {
            panic!("an image-bearing delivery must be binary");
        };
        let trailer = body.len() - 8;
        assert_eq!(&body[trailer + 4..], b"JTR1");
        let metadata_size =
            u32::from_le_bytes(body[trailer..trailer + 4].try_into().unwrap()) as usize;
        assert_eq!(trailer - metadata_size, 8);
        assert_eq!(&body[..8], &[255, 0, 0, 255, 0, 0, 255, 255]);
        let metadata: Value = serde_json::from_slice(&body[8..trailer]).unwrap();
        assert_eq!(metadata["image_bytes"], 4);
        assert_eq!(metadata["preview"]["image_bytes"], 4);
        assert_eq!(metadata["preview"]["layout"], 7);
        assert!(metadata["frame"]["images"][0].get("rgba").is_none());
        assert!(metadata["preview"]["frame"]["images"][0]
            .get("rgba")
            .is_none());
    }

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
        exercise_retained_ptys(false);
    }

    #[test]
    fn structured_frames_share_one_delivery_credit_for_active_and_passive_ptys() {
        exercise_retained_ptys(true);
    }

    fn exercise_retained_ptys(direct_frames: bool) {
        let format = if direct_frames {
            LiveFrameFormat::Dense
        } else {
            SnapshotFormat::Channel.into()
        };
        let decode = |packet: &Value| -> Value {
            if direct_frames {
                let frame = &packet["frame"];
                assert!(frame.is_object());
                assert_eq!(
                    packet["frame_bytes"],
                    serde_json::to_string(frame).unwrap().len()
                );
                frame.clone()
            } else {
                assert!(packet.get("frame_bytes").is_none());
                serde_json::from_str(packet["frame"].as_str().unwrap()).unwrap()
            }
        };
        let local = Arc::new(Mutex::new(LocalSessions::default()));
        let shared = Arc::downgrade(&local);
        let mut sessions = local.lock().unwrap();
        let first = sessions
            .open(
                false,
                SnapshotFormat::Channel.into(),
                None,
                None,
                shared.clone(),
            )
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
                        format: SnapshotFormat::Channel.into(),
                        history: None,
                        window_pixels: None,
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
                format,
                if direct_frames {
                    FrameDelivery::Structured
                } else {
                    FrameDelivery::Quoted
                },
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
            if event["preview"].is_object() {
                let preview = &event["preview"];
                let layout = preview["layout"].as_u64().unwrap();
                let frame = decode(preview);
                let expected = if layout == 71 {
                    &first
                } else {
                    assert_eq!(layout, 72);
                    &second
                };
                assert_eq!(frame["stamp"]["epoch"], expected.epoch.to_string());
                assert_eq!(frame["stamp"]["session"], expected.session);
                assert_eq!(frame["stamp"]["version"], format.channel_version().unwrap());
                if !seen.contains(&layout) {
                    seen.push(layout);
                }
            }
            assert!(
                receive.recv_timeout(Duration::from_millis(35)).is_err(),
                "second envelope before acknowledgement"
            );
            let revision = if event["frame"].is_null() {
                assert!(event.get("frame_bytes").is_none());
                None
            } else {
                let frame = decode(&event);
                assert_eq!(frame["stamp"]["version"], format.channel_version().unwrap());
                Some(serde_json::from_value(frame["stamp"]["revision"].clone()).unwrap())
            };
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
            .open(
                false,
                SnapshotFormat::Channel.into(),
                None,
                None,
                Arc::downgrade(&local),
            )
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
                SnapshotFormat::Channel.into(),
                FrameDelivery::Quoted,
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
            .recover(
                &owner,
                tauri_terminal_frame_bridge::SnapshotFormat::Channel.into(),
            )
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
