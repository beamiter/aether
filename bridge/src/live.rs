//! Linux-only limited PTY integration. Not a production process owner.
use crate::{from_engine, WireFrame};
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
    epoch: u32,
    flight: Option<u32>,
    delivered: u32,
    input: VecDeque<u8>,
}
impl LiveBridge {
    pub fn open(&mut self, rows: usize, cols: usize) -> Result<u32, String> {
        if self.session.is_some() {
            return Err("close the existing session first".into());
        }
        let epoch = self.epoch.checked_add(1).ok_or("epoch exhausted")?;
        let session = Session::spawn(rows, cols)?;
        self.epoch = epoch;
        self.session = Some(session);
        self.flight = None;
        self.delivered = 0;
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
    pub fn poll(&mut self, epoch: u32) -> Result<Option<WireFrame>, String> {
        self.check(epoch)?;
        let session = self.session.as_mut().unwrap();
        session.poll()?;
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
        let frame = session.terminal().frame()?;
        let revision = u32::try_from(frame.stamp().revision).map_err(|_| "revision exhausted")?;
        if revision == self.delivered {
            return Ok(None);
        }
        let wire = from_engine(&frame, "pty-demo", epoch)?;
        self.flight = Some(revision);
        Ok(Some(wire))
    }
    /// Explicit full-snapshot recovery after lost delivery/ACK. Never changes PTY or input.
    /// Caller must discard older outstanding transport callbacks before applying this frame.
    pub fn recover_frame(&mut self, epoch: u32) -> Result<WireFrame, String> {
        self.check(epoch)?;
        let frame = self.session.as_ref().unwrap().terminal().frame()?;
        let wire = from_engine(&frame, "pty-demo", epoch)?;
        self.flight = Some(wire.revision);
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
        if self.flight.is_none() && self.delivered == revision && revision != 0 {
            return Ok(()); // retry of a successfully applied but lost ACK response
        }
        if self.flight != Some(revision) {
            return Err("wrong frame ACK".into());
        }
        self.delivered = revision;
        self.flight = None;
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
        self.input.clear();
        self.flight = None;
        Ok(CloseStatus { cleanup_pending })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(frame.session, "pty-demo");
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
