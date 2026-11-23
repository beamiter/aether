mod wayland;

use companion_host::{HostGate, SharedHost, Token};
use gtk::{gdk, prelude::*};
use std::{ffi::c_void, ptr::NonNull};

#[derive(Clone, PartialEq)]
pub(super) struct Surface {
    window: gdk::Window,
    clock: gdk::FrameClock,
    native: NativeSurface,
}

#[derive(Clone, PartialEq)]
enum NativeSurface {
    Wayland(NonNull<c_void>),
    X11,
}

impl Surface {
    pub(super) fn for_area(area: &gtk::DrawingArea) -> Option<Self> {
        let window = area.window()?;
        if !area.is_mapped() || window.is_destroyed() {
            return None;
        }
        let native = if window.is::<gdkx11::X11Window>() {
            NativeSurface::X11
        } else {
            NativeSurface::Wayland(wayland::surface(&window)?)
        };
        Some(Self {
            native,
            window,
            clock: area.frame_clock()?,
        })
    }
}

enum Backend {
    Wayland(wayland::Receipts),
    X11 {
        drawn: gdk::Atom,
        timings: gdk::Atom,
    },
}

impl Backend {
    fn new(display: gdk::Display) -> Option<Self> {
        if display.is::<gdkx11::X11Display>() {
            Some(Self::X11 {
                drawn: gdk::Atom::intern("_NET_WM_FRAME_DRAWN"),
                timings: gdk::Atom::intern("_NET_WM_FRAME_TIMINGS"),
            })
        } else {
            wayland::Receipts::new(display).map(Self::Wayland)
        }
    }

    fn ready(&self, surface: &Surface) -> bool {
        match self {
            Self::Wayland(receipts) => receipts.ready(),
            Self::X11 { drawn, timings } => surface
                .window
                .screen()
                .downcast::<gdkx11::X11Screen>()
                .is_ok_and(|screen| {
                    screen.upcast_ref::<gdk::Screen>().is_composited()
                        && screen.supports_net_wm_hint(drawn)
                        && screen.supports_net_wm_hint(timings)
                }),
        }
    }

    fn track(&self, surface: &Surface, serial: u64) -> bool {
        match self {
            Self::Wayland(receipts) => receipts.request(&surface.window, serial).is_some(),
            Self::X11 { .. } => self.ready(surface),
        }
    }

    fn poll(&mut self, surface: Option<&Surface>, frames: &[PaintedFrame]) -> Vec<(u64, bool)> {
        match self {
            Self::Wayland(receipts) => receipts.poll(),
            Self::X11 { .. } => {
                if frames.is_empty() {
                    return Vec::new();
                }
                let Some(surface) = surface.filter(|surface| self.ready(surface)) else {
                    return frames.iter().map(|frame| (frame.serial, false)).collect();
                };
                frames
                    .iter()
                    .filter_map(|frame| {
                        let Some(timing) = surface.clock.timings(frame.counter) else {
                            return Some((frame.serial, false));
                        };
                        // GTK's X11 backend correlates extended WM frame timing
                        // messages with the submitted XSync frame cookie.
                        timing
                            .is_complete()
                            .then(|| (frame.serial, timing.presentation_time().is_some()))
                    })
                    .collect()
            }
        }
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
    evidence: Option<Evidence>,
}

struct Evidence {
    connection: u64,
    admission: u64,
    content: Content,
}

#[derive(Default)]
pub(super) struct Presentation {
    surface: Option<Surface>,
    backend: Option<Backend>,
    frames: Vec<PaintedFrame>,
    last_counter: Option<i64>,
    serial: u64,
    proven: bool,
    lease: Option<Token>,
}

impl Presentation {
    pub(super) fn new(display: gdk::Display) -> Self {
        Self {
            backend: Backend::new(display),
            ..Self::default()
        }
    }

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

    pub(super) fn supported_backend(&self) -> bool {
        self.backend.is_some()
    }

    pub(super) fn needs_draw(&self) -> bool {
        self.frames.len() < 8
            && !self.frames.iter().any(|frame| frame.evidence.is_some())
            && self
                .backend
                .as_ref()
                .zip(self.surface.as_ref())
                .is_some_and(|(backend, surface)| backend.ready(surface))
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
            if let Some(frame) = self
                .frames
                .iter_mut()
                .find(|frame| frame.counter == counter)
            {
                frame.evidence = None;
            }
        }
    }

    /// Track a full-slot paint until the native backend confirms presentation
    /// on the same window.
    pub(super) fn painted(&mut self, mut client: Option<&mut SharedHost>, body: Option<Token>) {
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
        if let Some(client) = client.as_deref_mut() {
            client.begin_frame(self.serial);
        }
        let evidence = self.evidence(client.as_ref().map(|client| &client.gate), body);
        if let Some(frame) = self
            .frames
            .iter_mut()
            .find(|frame| frame.counter == counter)
        {
            frame.evidence = evidence;
            return;
        }
        if evidence.is_some()
            && self.frames.len() < 8
            && self
                .backend
                .as_ref()
                .is_some_and(|backend| backend.track(surface, self.serial))
        {
            self.frames.push(PaintedFrame {
                counter,
                serial: self.serial,
                evidence,
            });
        }
    }

    fn evidence(&self, gate: Option<&HostGate>, body: Option<Token>) -> Option<Evidence> {
        let Some(gate) = gate else {
            return (!self.proven).then_some(Evidence {
                connection: 0,
                admission: 0,
                content: Content::Probe,
            });
        };
        let content = if let Some(token) = body {
            gate.body_presentation_pending()
                .then_some(Content::Body(token))?
        } else {
            let (token, revoked_after) = gate.pending_retirement_boundary()?;
            if self.serial <= revoked_after || !self.can_draw(token) {
                return None;
            }
            Content::Cleared {
                token: token.clone(),
                revoked_after,
            }
        };
        Some(Evidence {
            connection: gate.connection_generation(),
            admission: gate.admission_generation(),
            content,
        })
    }

    pub(super) fn presented(
        &mut self,
        mut client: Option<&mut SharedHost>,
        current: Option<&Surface>,
    ) {
        if let Some(client) = client.as_deref() {
            if self
                .lease
                .as_ref()
                .is_some_and(|token| client.gate.retirement_accepted(token))
            {
                self.lease = None;
            }
        }
        let same_surface = self.matches(current);
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        for (serial, presented) in backend.poll(current.filter(|_| same_surface), &self.frames) {
            let Some(index) = self.frames.iter().position(|frame| frame.serial == serial) else {
                continue;
            };
            let frame = self.frames.swap_remove(index);
            let Some(evidence) = frame.evidence.filter(|_| same_surface && presented) else {
                continue;
            };
            if matches!(evidence.content, Content::Probe) {
                self.proven = true;
            } else if let Some(client) = client.as_deref_mut() {
                evidence.deliver(client, serial);
            }
        }
    }
}

impl Evidence {
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

    fn deliver(self, client: &mut SharedHost, serial: u64) {
        if self.is_current(&client.gate) {
            if matches!(self.content, Content::Body(_)) {
                client.body_submitted(serial);
            }
            client.frame_presented(serial);
        }
    }
}
