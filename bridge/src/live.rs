//! Linux-only limited PTY integration. Not a production process owner.
use crate::{
    from_engine,
    semantic::{self, Attachment, Resume, Update},
    WireFrame,
};
use jterm_engine::frame::CellFrame;
use jterm_headless_session_spike::Session;
use std::collections::VecDeque;
#[derive(Debug, serde::Serialize)]
pub struct SessionStatus {
    pub epoch: u32,
    pub exited: bool,
    pub drained: bool,
    pub write_error: Option<String>,
    pub dropped_reply_bytes: String,
    pub unsupported: String,
}
#[derive(Debug, serde::Serialize)]
pub struct HistorySnapshot {
    pub epoch: u32,
    pub revision: String,
    pub offset: usize,
    pub total_rows: usize,
    pub text: String,
}
#[derive(Debug, serde::Serialize)]
pub struct CloseStatus {
    pub cleanup_pending: bool,
}
#[derive(Default)]
pub struct LiveBridge {
    session: Option<Session>,
    id: String,
    epoch: u32,
    generation: u32,
    input_attached: bool,
    resumed_from: Option<u32>,
    flight: Option<CellFrame>,
    accepted: Option<CellFrame>,
    input: VecDeque<u8>,
    stable: Option<CellFrame>,
}
impl LiveBridge {
    pub fn open(&mut self, rows: usize, cols: usize) -> Result<u32, String> {
        if self.session.is_some() {
            return Err("close the existing session first".into());
        }
        let epoch = self.epoch.checked_add(1).ok_or("epoch exhausted")?;
        let id = semantic::session_id()?;
        let session = Session::spawn(rows, cols)?;
        self.stable = Some(session.terminal().frame()?);
        self.epoch = epoch;
        self.id = id;
        self.generation = 1;
        self.input_attached = true;
        self.resumed_from = None;
        self.session = Some(session);
        self.flight = None;
        self.accepted = None;
        self.input.clear();
        Ok(epoch)
    }
    fn check(&self, epoch: u32) -> Result<(), String> {
        if epoch != self.epoch || self.session.is_none() {
            Err("stale or closed session".into())
        } else {
            Ok(())
        }
    }
    pub fn attachment(&self, epoch: u32) -> Result<Attachment, String> {
        self.check(epoch)?;
        Ok(Attachment {
            session: self.id.clone(),
            epoch,
            generation: self.generation,
        })
    }

    pub fn validate_session(&self, owner: &Attachment) -> Result<u32, String> {
        self.check(owner.epoch)?;
        if owner.session != self.id {
            return Err("foreign terminal session".into());
        }
        Ok(owner.epoch)
    }

    pub fn validate_attachment(&self, owner: &Attachment) -> Result<u32, String> {
        let epoch = self.validate_session(owner)?;
        if owner.generation != self.generation {
            return Err("stale terminal attachment".into());
        }
        Ok(epoch)
    }

    pub fn validate_input(&self, owner: &Attachment) -> Result<u32, String> {
        let epoch = self.validate_attachment(owner)?;
        if self.input_attached {
            Ok(epoch)
        } else {
            Err("reconnected terminal view is read-only".into())
        }
    }

    pub fn detach(&mut self, owner: &Attachment) -> Result<(), String> {
        self.validate_attachment(owner)?;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("attachment exhausted")?;
        self.input_attached = false;
        self.flight = None;
        self.accepted = None;
        self.resumed_from = None;
        Ok(())
    }

    /// A reconnect starts with a snapshot and no dependency on the old viewer's
    /// data. A retry of an unacknowledged recovery gets the same attachment.
    pub fn resume(&mut self, expected: Option<&Attachment>) -> Result<Option<Resume>, String> {
        if let Some(owner) = expected {
            self.validate_session(owner)?;
            if self.resumed_from == Some(owner.generation) && self.accepted.is_none() {
                if let Some(frame) = self.flight.as_ref() {
                    return Ok(Some(Resume {
                        owner: self.attachment(self.epoch)?,
                        frame: from_engine(frame, &self.id, self.epoch)?,
                    }));
                }
            }
            self.validate_attachment(owner)?;
        } else if self.session.is_none() {
            return Ok(None);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("attachment exhausted")?;
        let frame = self.recover_frame(self.epoch)?;
        self.generation = generation;
        self.input_attached &= expected.is_some();
        self.resumed_from = expected.map(|owner| owner.generation);
        self.accepted = None;
        Ok(Some(Resume {
            owner: self.attachment(self.epoch)?,
            frame,
        }))
    }

    fn poll_frame(&mut self, epoch: u32) -> Result<Option<CellFrame>, String> {
        self.check(epoch)?;
        let session = self.session.as_mut().unwrap();
        session.poll_with_sync_capture(self.flight.is_none())?;
        if !self.input.is_empty() && !session.is_exited() && session.write_error().is_none() {
            let limit = 4096.min(self.input.len());
            // A sticky writer failure must not stop read-side draining.
            if let Ok(n) = session.write(&self.input.make_contiguous()[..limit]) {
                self.input.drain(..n);
            }
        }
        if session.is_exited() || session.write_error().is_some() {
            self.input.clear();
        }
        if self.flight.is_some() {
            return Ok(None);
        }
        let completed = session.take_completed_sync_frame();
        let frame = if !session.is_drained() && session.terminal().publication_held() {
            match completed {
                Some(frame) => frame,
                None => return Ok(None),
            }
        } else {
            session.terminal().frame()?
        };
        if self
            .accepted
            .as_ref()
            .is_some_and(|last| frame.stamp().revision == last.stamp().revision)
        {
            return Ok(None);
        }
        Ok(Some(frame))
    }

    fn publish(&mut self, frame: CellFrame) {
        self.stable = Some(frame.clone());
        self.flight = Some(frame);
    }

    pub fn poll(&mut self, epoch: u32) -> Result<Option<WireFrame>, String> {
        let Some(frame) = self.poll_frame(epoch)? else {
            return Ok(None);
        };
        let wire = from_engine(&frame, &self.id, epoch)?;
        self.publish(frame);
        Ok(Some(wire))
    }

    pub fn poll_update(&mut self, epoch: u32) -> Result<Option<Update>, String> {
        let Some(frame) = self.poll_frame(epoch)? else {
            return Ok(None);
        };
        let update = semantic::update(self.accepted.as_ref(), &frame, &self.id, epoch)?;
        self.publish(frame);
        Ok(Some(update))
    }
    /// Explicit full-snapshot recovery after lost delivery/ACK. Never changes PTY or input.
    /// Caller must discard older outstanding transport callbacks before applying this frame.
    pub fn recover_frame(&mut self, epoch: u32) -> Result<WireFrame, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        let frame = if !session.is_drained() && session.terminal().publication_held() {
            self.stable.clone().ok_or("missing stable frame")?
        } else {
            session.terminal().frame()?
        };
        let wire = from_engine(&frame, &self.id, epoch)?;
        self.publish(frame);
        Ok(wire)
    }
    /// Independent immutable history snapshot; does not consume or acknowledge a live frame.
    pub fn history(&self, epoch: u32, offset: usize) -> Result<HistorySnapshot, String> {
        self.check(epoch)?;
        let page = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .history_page(offset, 128)?;
        Ok(HistorySnapshot {
            epoch,
            revision: page.revision.to_string(),
            offset: page.offset,
            total_rows: page.total_rows,
            text: page.text,
        })
    }
    pub fn status(&self, epoch: u32) -> Result<SessionStatus, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        Ok(SessionStatus {
            epoch,
            exited: session.is_exited(),
            drained: session.is_drained(),
            write_error: session.write_error().map(str::to_owned),
            dropped_reply_bytes: session.dropped_reply_bytes().to_string(),
            unsupported: session.terminal().unsupported_count().to_string(),
        })
    }
    pub fn acknowledge(&mut self, epoch: u32, revision: u32) -> Result<(), String> {
        self.check(epoch)?;
        if self.flight.is_none()
            && self
                .accepted
                .as_ref()
                .is_some_and(|frame| frame.stamp().revision == u64::from(revision))
        {
            return Ok(()); // retry of a successfully applied but lost ACK response
        }
        if self
            .flight
            .as_ref()
            .is_none_or(|frame| frame.stamp().revision != u64::from(revision))
        {
            return Err("wrong frame ACK".into());
        }
        self.accepted = self.flight.take();
        self.resumed_from = None;
        Ok(())
    }
    fn enqueue(&mut self, epoch: u32, bytes: Vec<u8>) -> Result<(), String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        if session.is_exited() || session.write_error().is_some() {
            return Err("input unavailable".into());
        }
        if self.input.len() + bytes.len() > 16384 {
            return Err("input queue full; event not accepted".into());
        }
        self.input.extend(bytes);
        Ok(())
    }
    pub fn key(
        &mut self,
        epoch: u32,
        name: &str,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Result<(), String> {
        self.check(epoch)?;
        let bytes = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .encode_key(name, shift, alt, ctrl)?;
        self.enqueue(epoch, bytes)
    }
    pub fn text(&mut self, epoch: u32, text: &str) -> Result<(), String> {
        self.check(epoch)?;
        let bytes = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .encode_text(text)?;
        self.enqueue(epoch, bytes)
    }
    pub fn paste(&mut self, epoch: u32, text: &str) -> Result<(), String> {
        self.check(epoch)?;
        let bytes = self
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .encode_paste(text)?;
        self.enqueue(epoch, bytes)
    }
    pub fn write(&mut self, epoch: u32, text: &[u8]) -> Result<usize, String> {
        self.check(epoch)?;
        if !self.input.is_empty() {
            return Ok(0);
        }
        self.session.as_mut().unwrap().write(text)
    }
    pub fn resize(&mut self, epoch: u32, rows: usize, cols: usize) -> Result<(), String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().resize(rows, cols)
    }
    pub fn close(&mut self, epoch: u32) -> Result<CloseStatus, String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().close()?;
        let cleanup_pending = self.session.as_ref().unwrap().cleanup_pending();
        self.session.take();
        self.id.clear();
        self.input_attached = false;
        self.stable = None;
        self.input.clear();
        self.flight = None;
        self.accepted = None;
        self.resumed_from = None;
        Ok(CloseStatus { cleanup_pending })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_stream_reconnects_to_the_same_pty_after_lost_delta_and_ack() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(16, 80).unwrap();
        let original = bridge.attachment(epoch).unwrap();
        let Some(Update::Snapshot { frame }) = bridge.poll_update(epoch).unwrap() else {
            panic!("missing initial snapshot");
        };
        bridge.acknowledge(epoch, frame.revision).unwrap();
        write_owned_line(
            &mut bridge,
            epoch,
            b"TOKEN=STILL_RUNNING; stty -echo; printf '\\033[10;1H%s\\r\\n' INITIAL_DELTA\n",
        );

        let mut accepted = frame.revision;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let unacknowledged = loop {
            assert!(
                std::time::Instant::now() < deadline,
                "visible delta missing"
            );
            match bridge.poll_update(epoch).unwrap() {
                Some(Update::Delta { frame }) => {
                    assert_eq!(frame.base_revision, accepted);
                    assert_eq!(frame.session, original.session);
                    assert!(frame.rows.len() < 16);
                    if frame.rows.iter().any(|row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.text.as_str())
                            .collect::<String>()
                            .trim()
                            == "INITIAL_DELTA"
                    }) {
                        let revision = frame.revision;
                        let delta_len = serde_json::to_vec(&Update::Delta { frame }).unwrap().len();
                        let current = bridge.flight.as_ref().unwrap();
                        let snapshot_len = serde_json::to_vec(&Update::Snapshot {
                            frame: from_engine(current, &original.session, epoch).unwrap(),
                        })
                        .unwrap()
                        .len();
                        assert!(delta_len < snapshot_len);
                        println!(
                            "{{\"delta_bytes\":{delta_len},\"snapshot_bytes\":{snapshot_len}}}"
                        );
                        break revision;
                    }
                    accepted = frame.revision;
                    bridge.acknowledge(epoch, accepted).unwrap();
                }
                Some(Update::Snapshot { frame }) => {
                    accepted = frame.revision;
                    bridge.acknowledge(epoch, accepted).unwrap();
                }
                None => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        };
        assert!(bridge.poll_update(epoch).unwrap().is_none());

        let resumed = bridge.resume(Some(&original)).unwrap().unwrap();
        assert_eq!(resumed.owner.session, original.session);
        assert_eq!(resumed.owner.epoch, original.epoch);
        assert_ne!(resumed.owner.generation, original.generation);
        assert!(resumed.frame.revision >= unacknowledged);
        let retry = bridge.resume(Some(&original)).unwrap().unwrap();
        assert_eq!(retry.owner, resumed.owner);
        assert_eq!(retry.frame.revision, resumed.frame.revision);
        assert!(bridge.validate_attachment(&original).is_err());
        assert!(bridge.validate_input(&original).is_err());
        bridge.acknowledge(epoch, resumed.frame.revision).unwrap();
        assert!(bridge.resume(Some(&original)).is_err());

        assert_eq!(bridge.validate_input(&resumed.owner).unwrap(), epoch);
        write_owned_line(
            &mut bridge,
            epoch,
            b"printf '\\033[12;1H%s\\r\\n' \"$TOKEN\"\n",
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "shell variable did not survive"
            );
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if frame.rows.iter().any(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .trim()
                        == "STILL_RUNNING"
                }) {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        bridge.resize(epoch, 12, 72).unwrap();
        let Some(Update::Snapshot { frame }) = bridge.poll_update(epoch).unwrap() else {
            panic!("layout change must send a snapshot");
        };
        assert_eq!((frame.rows.len(), frame.cols), (12, 72));
        bridge.detach(&resumed.owner).unwrap();
        assert!(bridge.validate_attachment(&resumed.owner).is_err());
        let view_only = bridge.resume(None).unwrap().unwrap();
        assert_eq!(view_only.owner.session, original.session);
        assert!(bridge.validate_input(&view_only.owner).is_err());
        assert_eq!(bridge.validate_attachment(&view_only.owner).unwrap(), epoch);
        bridge.close(epoch).unwrap();
    }

    fn write_owned_line(bridge: &mut LiveBridge, epoch: u32, line: &[u8]) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut remaining = line;
        while !remaining.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "test PTY write stalled"
            );
            let written = bridge.write(epoch, remaining).unwrap();
            remaining = &remaining[written..];
            if written == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    #[test]
    fn real_pty_completed_cycle_publishes_before_next_hold_timeout() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 80).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        bridge.acknowledge(epoch, first.revision).unwrap();
        let command=b"stty -echo; printf '\\033[2J\\033[H\\033[?2026h\\101\\033[?2026l\\033[?2026h\\102'; read release; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        let complete = loop {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                if bridge
                    .session
                    .as_ref()
                    .unwrap()
                    .terminal()
                    .publication_held()
                {
                    break frame;
                }
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(complete.rows[0][0].text, "A");
        assert_eq!(complete.rows[0][1].text, " ");
        assert!(bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .visible_text()
            .contains("AB"));
        assert!(bridge.poll(epoch).unwrap().is_none());
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert_eq!(complete.revision, recovered.revision);
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        let live = loop {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                break frame;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(live.revision > complete.revision);
        assert_eq!(live.rows[0][1].text, "B");
        assert!(bridge.acknowledge(epoch, complete.revision).is_err());
        bridge.acknowledge(epoch, live.revision).unwrap();
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_sync_holds_publication_but_drains_and_recovers_stable() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        bridge.acknowledge(epoch, first.revision).unwrap();
        let command =
            b"stty -echo; printf '\\033[?2026h\\123\\131\\116\\103\\137\\120\\101\\122\\124'; read release; printf '\\123\\131\\116\\103\\137\\105\\116\\104\\033[?2026l'; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .publication_held()
        {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .visible_text()
            .contains("SYNC_PART"));
        let stable = bridge.stable.as_ref().unwrap().stamp().revision as u32;
        assert!(bridge.poll(epoch).unwrap().is_none());
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert_eq!(recovered.revision, stable);
        assert!(!recovered.rows.iter().any(|r| r
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .contains("SYNC_PART")));
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        assert!(bridge.recover_frame(epoch + 1).is_err());
        write_owned_line(&mut bridge, epoch, b"done\n");
        let mut released = false;
        while !bridge.status(epoch).unwrap().drained {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                released |= frame.rows.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("SYNC_PARTSYNC_END")
                });
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(released);
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_pressure_lost_ack_and_full_recovery() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        let command = b"i=0; while [ $i -lt 1400 ]; do printf 'PRESSURE_%s\\n' \"$i\"; i=$((i+1)); done; printf 'FINAL_SENTINEL\\n'; exit\n";
        let mut at = 0;
        let deadline = Instant::now() + Duration::from_secs(15);
        while !bridge.status(epoch).unwrap().drained {
            assert!(Instant::now() < deadline, "PTY did not drain");
            if at < command.len() {
                at += bridge.write(epoch, &command[at..]).unwrap();
            }
            // Withheld ACK never stops bounded PTY draining or allocates queued frames.
            assert!(bridge.poll(epoch).unwrap().is_none());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(at, command.len());
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert!(recovered.revision > first.revision);
        assert!(recovered.rows.iter().any(|row| row
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .contains("FINAL_SENTINEL")));
        assert_eq!(bridge.history(epoch, 0).unwrap().total_rows, 1024);
        assert!(bridge.acknowledge(epoch, first.revision).is_err());
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        assert!(bridge.poll(epoch).unwrap().is_none());
        assert!(bridge.recover_frame(epoch + 1).is_err());
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_begin_before_first_publication_recovers_initial_frame() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 40).unwrap();
        let cmd = b"printf '\\033[?2026h'; read release; exit\n";
        write_owned_line(&mut bridge, epoch, cmd);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !bridge
            .session
            .as_ref()
            .unwrap()
            .terminal()
            .publication_held()
        {
            assert!(Instant::now() < deadline);
            bridge.session.as_mut().unwrap().poll().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(bridge.poll(epoch).unwrap().is_none());
        let frame = bridge.recover_frame(epoch).unwrap();
        assert_eq!(frame.revision, 1);
        bridge.acknowledge(epoch, frame.revision).unwrap();
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_final_drain_releases_unclosed_hold() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 40).unwrap();
        let command = b"printf '\\033[?2026h\\104\\122\\101\\111\\116'; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut drain_published = false;
        while !bridge.status(epoch).unwrap().drained {
            assert!(Instant::now() < deadline);
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                let has_final = frame.rows.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("DRAIN")
                });
                if has_final {
                    assert!(bridge.status(epoch).unwrap().drained);
                    drain_published = true;
                }
                bridge.acknowledge(epoch, frame.revision).unwrap();
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            drain_published,
            "the final poll must paint before the client stops polling"
        );
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn real_pty_missing_end_releases_while_child_still_running() {
        use std::time::{Duration, Instant};
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 48).unwrap();
        let start = Instant::now();
        write_owned_line(
            &mut bridge,
            epoch,
            b"printf '\\033[?2026h\\124\\111\\115\\105\\117\\125\\124'; read release; exit\n",
        );
        loop {
            assert!(start.elapsed() < Duration::from_secs(3));
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if frame.rows.iter().any(|r| {
                    r.iter()
                        .map(|c| c.text.as_str())
                        .collect::<String>()
                        .contains("TIMEOUT")
                }) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(start.elapsed() >= Duration::from_millis(250));
        assert!(!bridge.status(epoch).unwrap().exited);
        bridge.close(epoch).unwrap();
    }
    #[test]
    fn queue_admission_is_bounded_atomic_and_epoch_scoped() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        bridge.enqueue(epoch, vec![b'a'; 16384]).unwrap();
        assert!(bridge.key(epoch, "a", false, false, false).is_err());
        assert_eq!(bridge.input.len(), 16384);
        assert!(bridge.paste(epoch + 1, "hello").is_err());
        bridge.close(epoch).unwrap();
        assert!(bridge.input.is_empty());
        let next = bridge.open(8, 64).unwrap();
        assert!(bridge.key(epoch, "a", false, false, false).is_err());
        bridge.key(next, "ArrowUp", false, false, false).unwrap();
        assert_eq!(bridge.input.iter().copied().collect::<Vec<_>>(), b"\x1b[A");
        bridge.close(next).unwrap();
    }
    #[test]
    fn lifecycle_epoch_and_one_flight() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        assert!(bridge.open(8, 64).is_err());
        assert!(bridge.write(epoch + 1, b"x").is_err());
        assert!(bridge.status(epoch + 1).is_err());
        assert!(bridge.history(epoch + 1, 0).is_err());
        assert!(bridge.history(epoch, usize::MAX).is_err());
        assert_eq!(bridge.history(epoch, 0).unwrap().epoch, epoch);
        assert_eq!(bridge.status(epoch).unwrap().unsupported, "0");
        let frame = bridge.poll(epoch).unwrap().unwrap();
        assert_eq!(frame.session, bridge.attachment(epoch).unwrap().session);
        assert!(bridge.poll(epoch).unwrap().is_none());
        bridge.history(epoch, 0).unwrap();
        assert!(bridge.poll(epoch).unwrap().is_none());
        assert!(bridge.acknowledge(epoch, frame.revision + 1).is_err());
        bridge.acknowledge(epoch, frame.revision).unwrap();
        bridge.close(epoch).unwrap();
        assert!(bridge.write(epoch, b"x").is_err());
        let next = bridge.open(8, 64).unwrap();
        assert!(next > epoch);
        assert!(bridge.poll(epoch).is_err());
        bridge.close(next).unwrap();
    }
}
