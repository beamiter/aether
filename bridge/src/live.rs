//! Linux-only limited PTY integration. Not a production process owner.
use crate::{from_engine, WireFrame, WireMouse};
use jterm_headless_session_spike::{MouseButton, MouseIntent, Presentation, Session, Terminal};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const RESIZE_REPAINT_QUIET: Duration = Duration::from_millis(120);
const RESIZE_REPAINT_LIMIT: Duration = Duration::from_secs(1);
const INPUT_LIMIT: usize = 16384;
// A reported press reserves room for one matching release.
const RELEASE_RESERVE: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MouseFence {
    layout: u32,
    generation: u32,
}

impl MouseFence {
    fn matches(self, terminal: &Terminal) -> bool {
        u64::from(self.layout) == terminal.layout()
            && self.generation == terminal.mouse_state().generation
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum MouseAction {
    Press,
    Release,
    Motion,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum PointerButton {
    Left,
    Middle,
    Right,
}

impl From<PointerButton> for MouseButton {
    fn from(button: PointerButton) -> Self {
        match button {
            PointerButton::Left => Self::Left,
            PointerButton::Middle => Self::Middle,
            PointerButton::Right => Self::Right,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MouseInput {
    layout: u32,
    generation: u32,
    kind: MouseAction,
    button: Option<PointerButton>,
    row: usize,
    col: usize,
    alt: bool,
    ctrl: bool,
}

struct InputSpan {
    len: usize,
    mouse: Option<MouseFence>,
}

#[derive(Default)]
struct PendingInput {
    bytes: VecDeque<u8>,
    spans: VecDeque<InputSpan>,
}

impl PendingInput {
    fn extend(&mut self, bytes: Vec<u8>, mouse: Option<MouseFence>) {
        if bytes.is_empty() {
            return;
        }
        let len = bytes.len();
        if let Some(last) = self
            .spans
            .back_mut()
            .filter(|span| span.mouse.is_none() && mouse.is_none())
        {
            last.len += len;
        } else {
            self.spans.push_back(InputSpan { len, mouse });
        }
        self.bytes.extend(bytes);
    }

    fn flush(&mut self, session: &mut Session) {
        let mut budget = 4096;
        // A stale report whose first byte wasn't sent disappears as a unit.
        // After any write, complete its suffix to avoid a partial escape.
        for _ in 0..64 {
            let Some(span) = self.spans.front_mut() else {
                break;
            };
            if span
                .mouse
                .is_some_and(|fence| !fence.matches(session.terminal()))
            {
                self.bytes.drain(..span.len);
                self.spans.pop_front();
                continue;
            }
            if budget == 0 {
                break;
            }
            let limit = budget.min(span.len);
            let Ok(n @ 1..) = session.write(&self.bytes.make_contiguous()[..limit]) else {
                break;
            };
            self.bytes.drain(..n);
            budget -= n;
            span.len -= n;
            span.mouse = None;
            if span.len == 0 {
                self.spans.pop_front();
            }
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.spans.clear();
    }
}

fn live_frame(presentation: &Presentation, epoch: u32) -> Result<WireFrame, String> {
    let mut wire = from_engine(&presentation.frame, "pty-demo", epoch)?;
    wire.mouse = Some(WireMouse {
        tracking: presentation.mouse.tracking.label(),
        generation: presentation.mouse.generation,
    });
    Ok(wire)
}

struct ResizeRepaint {
    quiet_at: Instant,
    limit: Instant,
    completed_sync: Option<Presentation>,
}

impl ResizeRepaint {
    fn new(now: Instant) -> Self {
        Self {
            quiet_at: now + RESIZE_REPAINT_QUIET,
            limit: now + RESIZE_REPAINT_LIMIT,
            completed_sync: None,
        }
    }

    fn note_output(&mut self, now: Instant) {
        self.quiet_at = (now + RESIZE_REPAINT_QUIET).min(self.limit);
    }

    fn held(&self, now: Instant) -> bool {
        now < self.quiet_at
    }
}
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
    input: PendingInput,
    pressed_mouse: Option<(MouseFence, MouseButton)>,
    stable: Option<Presentation>,
    resize_repaint: Option<ResizeRepaint>,
}
impl LiveBridge {
    pub fn open(&mut self, rows: usize, cols: usize) -> Result<u32, String> {
        if self.session.is_some() {
            return Err("close the existing session first".into());
        }
        let epoch = self.epoch.checked_add(1).ok_or("epoch exhausted")?;
        let session = Session::spawn(rows, cols)?;
        self.stable = Some(session.terminal().presentation()?);
        self.epoch = epoch;
        self.session = Some(session);
        self.flight = None;
        self.resize_repaint = None;
        self.delivered = 0;
        self.input.clear();
        self.pressed_mouse = None;
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
        self.poll_at(epoch, Instant::now())
    }
    fn poll_at(&mut self, epoch: u32, now: Instant) -> Result<Option<WireFrame>, String> {
        self.check(epoch)?;
        let session = self.session.as_mut().unwrap();
        let bytes = session.poll_with_sync_capture(self.flight.is_none())?;
        if !session.is_exited() && session.write_error().is_none() {
            self.input.flush(session);
        }
        if self
            .pressed_mouse
            .is_some_and(|(fence, _)| !fence.matches(session.terminal()))
        {
            self.pressed_mouse = None;
        }
        if session.is_exited() || session.write_error().is_some() {
            self.input.clear();
            self.pressed_mouse = None;
        }
        let mut completed = session.take_completed_sync_frame();
        if let Some(repaint) = &mut self.resize_repaint {
            if bytes != 0 {
                repaint.note_output(now);
            }
            if completed.is_some() {
                repaint.completed_sync = completed.take();
            }
            if !session.is_drained() && repaint.held(now) {
                return Ok(None);
            }
            completed = self
                .resize_repaint
                .take()
                .and_then(|hold| hold.completed_sync);
        }
        if self.flight.is_some() {
            return Ok(None);
        }
        let frame = if !session.is_drained() && session.terminal().publication_held() {
            match completed {
                Some(frame) => frame,
                None => return Ok(None),
            }
        } else {
            session.terminal().presentation()?
        };
        let revision =
            u32::try_from(frame.frame.stamp().revision).map_err(|_| "revision exhausted")?;
        if revision == self.delivered {
            return Ok(None);
        }
        let wire = live_frame(&frame, epoch)?;
        self.flight = Some(revision);
        self.stable = Some(frame);
        Ok(Some(wire))
    }
    /// Explicit full-snapshot recovery after lost delivery/ACK. Never changes PTY or input.
    /// Caller must discard older outstanding transport callbacks before applying this frame.
    pub fn recover_frame(&mut self, epoch: u32) -> Result<WireFrame, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        if !session.is_drained()
            && (self.resize_repaint.is_some() || session.terminal().publication_held())
        {
            let wire = live_frame(self.stable.as_ref().ok_or("missing stable frame")?, epoch)?;
            self.flight = Some(wire.revision);
            return Ok(wire);
        }
        let frame = session.terminal().presentation()?;
        let wire = live_frame(&frame, epoch)?;
        self.flight = Some(wire.revision);
        self.stable = Some(frame);
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
        let reserve = self.pressed_mouse.map_or(0, |_| RELEASE_RESERVE);
        if self.input.bytes.len() + bytes.len() + reserve > INPUT_LIMIT {
            return Err("input queue full; event not accepted".into());
        }
        self.input.extend(bytes, None);
        Ok(())
    }
    pub fn mouse(&mut self, epoch: u32, report: MouseInput) -> Result<bool, String> {
        self.check(epoch)?;
        let session = self.session.as_ref().unwrap();
        let fence = MouseFence {
            layout: report.layout,
            generation: report.generation,
        };
        if session.is_exited()
            || session.write_error().is_some()
            || !fence.matches(session.terminal())
            || !self.stable.as_ref().is_some_and(|stable| {
                stable.frame.stamp().layout == u64::from(fence.layout)
                    && stable.mouse.generation == fence.generation
            })
        {
            return Ok(false);
        }
        let owned = self
            .pressed_mouse
            .filter(|(owner, _)| *owner == fence)
            .map(|(_, button)| button);
        let button = report.button.map(MouseButton::from);
        let intent = match (report.kind, button) {
            (MouseAction::Press, Some(button)) if owned.is_none() => MouseIntent::Press(button),
            (MouseAction::Release, Some(button)) if owned == Some(button) => {
                MouseIntent::Release(button)
            }
            (MouseAction::Motion, button) if owned == button => MouseIntent::Motion(button),
            (MouseAction::WheelUp, None) => MouseIntent::WheelUp,
            (MouseAction::WheelDown, None) => MouseIntent::WheelDown,
            _ => return Ok(false),
        };
        let Some(bytes) = session.terminal().encode_mouse(
            intent,
            report.row,
            report.col,
            report.alt,
            report.ctrl,
        ) else {
            return Ok(false);
        };
        let reserve = if matches!(intent, MouseIntent::Press(_))
            || (owned.is_some() && !matches!(intent, MouseIntent::Release(_)))
        {
            RELEASE_RESERVE
        } else {
            0
        };
        if self.input.bytes.len() + bytes.len() + reserve > INPUT_LIMIT {
            return Ok(false);
        }
        self.input.extend(bytes, Some(fence));
        match intent {
            MouseIntent::Press(button) => self.pressed_mouse = Some((fence, button)),
            MouseIntent::Release(_) => self.pressed_mouse = None,
            _ => (),
        }
        Ok(true)
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
        if !self.input.bytes.is_empty() {
            return Ok(0);
        }
        self.session.as_mut().unwrap().write(text)
    }
    pub fn resize(&mut self, epoch: u32, rows: usize, cols: usize) -> Result<(), String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().resize(rows, cols)?;
        let now = Instant::now();
        match &mut self.resize_repaint {
            Some(repaint) => {
                repaint.note_output(now);
                repaint.completed_sync = None;
            }
            None => self.resize_repaint = Some(ResizeRepaint::new(now)),
        }
        Ok(())
    }
    pub fn close(&mut self, epoch: u32) -> Result<CloseStatus, String> {
        self.check(epoch)?;
        self.session.as_mut().unwrap().close()?;
        let cleanup_pending = self.session.as_ref().unwrap().cleanup_pending();
        self.session.take();
        self.stable = None;
        self.resize_repaint = None;
        self.input.clear();
        self.pressed_mouse = None;
        self.flight = None;
        Ok(CloseStatus { cleanup_pending })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

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
    fn resize_repaint_coalesces_output_until_quiet_with_a_hard_limit() {
        let start = Instant::now();
        let mut repaint = ResizeRepaint::new(start);
        assert!(repaint.held(start));
        assert!(!repaint.held(start + RESIZE_REPAINT_QUIET));
        repaint.note_output(start + RESIZE_REPAINT_QUIET / 2);
        assert!(repaint.held(start + RESIZE_REPAINT_QUIET));
        repaint.note_output(start + RESIZE_REPAINT_LIMIT - Duration::from_millis(1));
        assert!(!repaint.held(start + RESIZE_REPAINT_LIMIT));
    }

    #[test]
    fn real_pty_resize_drains_at_new_geometry_while_recovery_keeps_stable_frame() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(8, 64).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        bridge.acknowledge(epoch, first.revision).unwrap();
        bridge.resize(epoch, 9, 78).unwrap();
        bridge.resize(epoch, 10, 80).unwrap();
        let limit = bridge.resize_repaint.as_ref().unwrap().limit;
        let start = limit - RESIZE_REPAINT_LIMIT;
        let command = b"stty -echo; printf 'GRID:'; stty size; read release; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(
                Instant::now() < deadline,
                "resized shell output did not arrive"
            );
            assert!(bridge.poll_at(epoch, start).unwrap().is_none());
            if bridge
                .session
                .as_ref()
                .unwrap()
                .terminal()
                .visible_text()
                .contains("GRID:10 80")
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let recovered = bridge.recover_frame(epoch).unwrap();
        assert_eq!(recovered.revision, first.revision);
        assert_eq!((recovered.rows.len(), recovered.cols), (8, 64));
        bridge.acknowledge(epoch, recovered.revision).unwrap();
        let resized = bridge.poll_at(epoch, limit).unwrap().unwrap();
        assert_eq!((resized.rows.len(), resized.cols), (10, 80));
        assert!(resized.layout > first.layout);
        assert!(bridge.resize_repaint.is_none());
        bridge.acknowledge(epoch, resized.revision).unwrap();
        bridge.close(epoch).unwrap();
    }

    #[test]
    fn real_pty_resize_release_uses_completed_sync_frame_instead_of_partial_repaint() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(4, 80).unwrap();
        let first = bridge.poll(epoch).unwrap().unwrap();
        bridge.acknowledge(epoch, first.revision).unwrap();
        bridge.resize(epoch, 5, 82).unwrap();
        let limit = bridge.resize_repaint.as_ref().unwrap().limit;
        let start = limit - RESIZE_REPAINT_LIMIT;
        let command = b"stty -echo; printf '\\033[2J\\033[H\\033[?2026h\\101\\033[?2026l\\033[?2026h\\102'; read release; exit\n";
        write_owned_line(&mut bridge, epoch, command);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(
                Instant::now() < deadline,
                "synchronized output did not arrive"
            );
            assert!(bridge.poll_at(epoch, start).unwrap().is_none());
            let terminal = bridge.session.as_ref().unwrap().terminal();
            if terminal.publication_held() && terminal.visible_text().contains("AB") {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let complete = bridge.poll_at(epoch, limit).unwrap().unwrap();
        assert_eq!((complete.rows.len(), complete.cols), (5, 82));
        assert_eq!(complete.rows[0][0].text, "A");
        assert_eq!(complete.rows[0][1].text, " ");
        bridge.acknowledge(epoch, complete.revision).unwrap();
        bridge.close(epoch).unwrap();
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
        let stable = bridge.stable.as_ref().unwrap().frame.stamp().revision as u32;
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
        assert_eq!(bridge.input.bytes.len(), 16384);
        assert!(bridge.paste(epoch + 1, "hello").is_err());
        bridge.close(epoch).unwrap();
        assert!(bridge.input.bytes.is_empty());
        let next = bridge.open(8, 64).unwrap();
        assert!(bridge.key(epoch, "a", false, false, false).is_err());
        bridge.key(next, "ArrowUp", false, false, false).unwrap();
        assert_eq!(
            bridge.input.bytes.iter().copied().collect::<Vec<_>>(),
            b"\x1b[A"
        );
        bridge.close(next).unwrap();
    }
    fn rendered_mouse_mode(bridge: &mut LiveBridge, epoch: u32) -> WireFrame {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "mouse mode was not published");
            if let Some(frame) = bridge.poll(epoch).unwrap() {
                bridge.acknowledge(epoch, frame.revision).unwrap();
                if frame
                    .mouse
                    .as_ref()
                    .is_some_and(|mouse| mouse.tracking == "button")
                {
                    return frame;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn mouse_input(frame: &WireFrame, kind: MouseAction) -> MouseInput {
        MouseInput {
            layout: frame.layout,
            generation: frame.mouse.as_ref().unwrap().generation,
            kind,
            button: Some(PointerButton::Left),
            row: 2,
            col: 4,
            alt: false,
            ctrl: true,
        }
    }
    #[test]
    fn mouse_reports_are_scoped_to_the_painted_layout_and_reserve_release_space() {
        let mut bridge = LiveBridge::default();
        let epoch = bridge.open(6, 80).unwrap();
        write_owned_line(
            &mut bridge,
            epoch,
            b"stty -echo; printf '\\033[?1000;1006h'; read release; exit\n",
        );
        let frame = rendered_mouse_mode(&mut bridge, epoch);
        let mut foreign = mouse_input(&frame, MouseAction::Press);
        foreign.generation += 1;
        assert!(!bridge.mouse(epoch, foreign).unwrap());
        assert!(bridge
            .mouse(epoch + 1, mouse_input(&frame, MouseAction::Press))
            .is_err());
        assert!(!bridge
            .mouse(epoch, mouse_input(&frame, MouseAction::Release))
            .unwrap());
        assert!(bridge
            .mouse(epoch, mouse_input(&frame, MouseAction::Press))
            .unwrap());
        let queued = bridge.input.bytes.len();
        bridge
            .enqueue(epoch, vec![b'x'; INPUT_LIMIT - RELEASE_RESERVE - queued])
            .unwrap();
        assert!(bridge.key(epoch, "a", false, false, false).is_err());
        assert!(bridge
            .mouse(epoch, mouse_input(&frame, MouseAction::Release))
            .unwrap());
        bridge.input.clear();
        assert!(bridge
            .mouse(epoch, mouse_input(&frame, MouseAction::Press))
            .unwrap());
        bridge.enqueue(epoch, vec![b'x']).unwrap();
        assert!(bridge
            .mouse(epoch, mouse_input(&frame, MouseAction::Release))
            .unwrap());
        // Start a real resize before the first queued report was written.
        bridge.resize(epoch, 7, 81).unwrap();
        assert!(!bridge
            .mouse(epoch, mouse_input(&frame, MouseAction::Press))
            .unwrap());
        let session = bridge.session.as_mut().unwrap();
        // Retain following text but discard stale mouse reports as complete spans.
        bridge.input.flush(session);
        assert!(bridge.input.spans.iter().all(|span| span.mouse.is_none()));
        assert!(bridge.input.bytes.iter().all(|byte| *byte == b'x'));
        bridge.close(epoch).unwrap();
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
