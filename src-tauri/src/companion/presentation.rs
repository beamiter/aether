use companion_host::{HostGate, SharedHost, Token};
use gtk::{gdk, prelude::*};
use std::collections::VecDeque;

#[derive(Clone, PartialEq)]
pub(super) struct Surface {
    window: gdk::Window,
    clock: gdk::FrameClock,
}

impl Surface {
    pub(super) fn for_area(area: &gtk::DrawingArea) -> Option<Self> {
        let window = area.window()?;
        if !area.is_mapped() || window.is_destroyed() {
            return None;
        }
        Some(Self {
            window,
            clock: area.frame_clock()?,
        })
    }
}

enum Content {
    Probe,
    Body(Token),
    Cleared { token: Token, revoked_after: u64 },
}

struct PaintedFrame {
    counter: i64,
    serial: u64,
    connection: u64,
    admission: u64,
    content: Content,
}

#[derive(Default)]
pub(super) struct Presentation {
    surface: Option<Surface>,
    frames: VecDeque<PaintedFrame>,
    last_counter: Option<i64>,
    serial: u64,
    proven: bool,
    lease: Option<Token>,
}

impl Presentation {
    pub(super) fn select(&mut self, surface: Option<Surface>, registered: bool) -> bool {
        // A registered host cannot claim that a different window cleared its body.
        if registered || self.surface == surface {
            return false;
        }
        self.surface = surface;
        self.frames.clear();
        self.last_counter = None;
        self.proven = false;
        true
    }

    pub(super) fn matches(&self, surface: Option<&Surface>) -> bool {
        surface.is_some() && self.surface.as_ref() == surface
    }

    pub(super) fn proven(&self) -> bool {
        self.proven
    }

    pub(super) fn can_draw(&self, token: &Token) -> bool {
        self.lease.as_ref().is_none_or(|held| held == token)
    }

    pub(super) fn holds_body(&self) -> bool {
        self.lease.is_some()
    }

    pub(super) fn mark_body_drawn(&mut self, token: &Token) {
        self.lease = Some(token.clone());
    }

    pub(super) fn discard_current_frame(&mut self) {
        if let Some(surface) = &self.surface {
            let counter = surface.clock.frame_counter();
            self.frames.retain(|frame| frame.counter != counter);
        }
    }

    /// Called during GTK drawing, after painting the dedicated slot. Timings
    /// must later prove that the same native frame reached presentation.
    pub(super) fn painted(&mut self, client: Option<&mut SharedHost>, body: Option<Token>) {
        let Some(surface) = &self.surface else { return };
        let counter = surface.clock.frame_counter();
        if counter < 0 || self.last_counter.is_some_and(|previous| counter < previous) {
            return;
        }
        if self.last_counter != Some(counter) {
            let Some(serial) = self.serial.checked_add(1) else {
                return;
            };
            self.serial = serial;
            self.last_counter = Some(counter);
        }
        self.frames.retain(|frame| frame.counter != counter);
        let (connection, admission, content) = if let Some(client) = client {
            client.begin_frame(self.serial);
            let gate = &client.gate;
            let content = if let Some(token) = body {
                if !gate.body_presentation_pending() {
                    return;
                }
                Content::Body(token)
            } else {
                let Some((token, revoked_after)) = gate.pending_retirement_boundary() else {
                    return;
                };
                if self.serial <= revoked_after || !self.can_draw(token) {
                    return;
                }
                Content::Cleared {
                    token: token.clone(),
                    revoked_after,
                }
            };
            (
                gate.connection_generation(),
                gate.admission_generation(),
                content,
            )
        } else if !self.proven {
            (0, 0, Content::Probe)
        } else {
            return;
        };
        if self.frames.len() < 8 {
            self.frames.push_back(PaintedFrame {
                counter,
                serial: self.serial,
                connection,
                admission,
                content,
            });
        }
    }

    pub(super) fn presented(&mut self, mut client: Option<&mut SharedHost>) {
        if let Some(client) = client.as_deref() {
            if self
                .lease
                .as_ref()
                .is_some_and(|token| client.gate.retirement_accepted(token))
            {
                self.lease = None;
            }
        }
        let Some(surface) = &self.surface else { return };
        if surface.window.is_destroyed() {
            self.frames.clear();
            return;
        }
        let mut remaining = VecDeque::new();
        for frame in self.frames.drain(..) {
            let Some(timing) = surface.clock.timings(frame.counter) else {
                continue;
            };
            if !timing.is_complete() {
                remaining.push_back(frame);
                continue;
            }
            if timing.presentation_time().is_none() {
                continue;
            }
            if matches!(frame.content, Content::Probe) {
                self.proven = true;
            } else if let Some(client) = client.as_deref_mut() {
                frame.deliver(client);
            }
        }
        self.frames = remaining;
    }
}

impl PaintedFrame {
    fn is_current(&self, gate: &HostGate) -> bool {
        if self.connection != gate.connection_generation()
            || self.admission != gate.admission_generation()
        {
            return false;
        }
        match &self.content {
            Content::Body(token) => {
                gate.body_presentation_pending() && gate.accepts_callback(token)
            }
            Content::Cleared {
                token,
                revoked_after,
            } => gate.pending_retirement_boundary() == Some((token, *revoked_after)),
            Content::Probe => false,
        }
    }

    fn deliver(self, client: &mut SharedHost) {
        if self.is_current(&client.gate) {
            if matches!(self.content, Content::Body(_)) {
                client.body_submitted(self.serial);
            }
            client.frame_presented(self.serial);
        }
    }
}
