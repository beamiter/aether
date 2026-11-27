use crate::protocol_v4::Counter;
use jterm_engine::{
    frame::Stamp,
    mouse::{MouseInput, MouseTracking},
    terminal::Terminal,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Tracking {
    PressOnly,
    Button,
    Drag,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MouseView {
    layout: Counter,
    revision: Counter,
    generation: u32,
    tracking: Tracking,
}

/// Wheel-only alternate-screen scrolling; separate from negotiated mouse tracking.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AlternateScrollView {
    layout: Counter,
    revision: Counter,
    generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Press,
    Release,
    Motion,
    WheelUp,
    WheelDown,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct MouseReport {
    layout: Counter,
    revision: Counter,
    generation: u32,
    row: usize,
    col: usize,
    kind: Action,
    button: Option<Button>,
    alt: bool,
    ctrl: bool,
}

#[derive(Default)]
pub(super) struct Mouse {
    pressed: Option<Pressed>,
}

#[derive(Clone, Copy)]
struct Pressed {
    stamp: Stamp,
    generation: u32,
    button: Button,
}

impl Mouse {
    pub(super) fn retire(&mut self) {
        self.pressed = None;
    }

    pub(super) fn view(&self, terminal: &Terminal, presented: Stamp) -> Option<MouseView> {
        let tracking = match terminal.mouse_report_profile()?.tracking {
            MouseTracking::PressOnly => Tracking::PressOnly,
            MouseTracking::Button => Tracking::Button,
            MouseTracking::Drag => Tracking::Drag,
            MouseTracking::Any => Tracking::Any,
        };
        let current = terminal.frame().ok()?.stamp();
        if presented != current || terminal.publication_held() {
            return None;
        }
        Some(MouseView {
            layout: Counter::new(presented.layout)?,
            revision: Counter::new(presented.revision)?,
            generation: generation(terminal)?,
            tracking,
        })
    }

    pub(super) fn alternate_view(
        &self,
        terminal: &Terminal,
        presented: Stamp,
    ) -> Option<AlternateScrollView> {
        if !terminal.alternate_scroll_active()
            || terminal.publication_held()
            || terminal.frame().ok()?.stamp() != presented
        {
            return None;
        }
        Some(AlternateScrollView {
            layout: Counter::new(presented.layout)?,
            revision: Counter::new(presented.revision)?,
            generation: generation(terminal)?,
        })
    }

    pub(super) fn presented_stamp(
        &self,
        report: &MouseReport,
        mut published: impl Iterator<Item = Stamp>,
    ) -> Option<Stamp> {
        let matches = |stamp: &Stamp| {
            report.revision.get() == stamp.revision && report.layout.get() == stamp.layout
        };
        // A release may refer to the surface that accepted its press even
        // after that exact publication leaves the bounded frame ledger.
        if let Some(held) = self.pressed.filter(|held| {
            matches!(report.kind, Action::Release)
                && report.button == Some(held.button)
                && report.generation == held.generation
                && matches(&held.stamp)
        }) {
            return Some(held.stamp);
        }
        published.find(matches)
    }

    pub(super) fn send(
        &mut self,
        session: &mut jterm_headless_session_spike::Session,
        presented: Stamp,
        report: MouseReport,
    ) -> Result<bool, String> {
        let terminal = session.terminal();
        let profile = terminal.mouse_report_profile();
        if profile.is_none()
            && (!terminal.alternate_scroll_active()
                || !matches!(report.kind, Action::WheelUp | Action::WheelDown)
                || report.button.is_some())
        {
            return Ok(false);
        }
        if generation(terminal) != Some(report.generation) {
            return Ok(false);
        }
        let current = terminal.frame()?.stamp();
        if !same_surface(presented, current) {
            return Ok(false);
        }
        let held = self.pressed.filter(|held| {
            held.generation == report.generation && same_surface(held.stamp, presented)
        });
        let input = match (report.kind, report.button) {
            (Action::Press, Some(button)) if held.is_none() => MouseInput::Press(button.code()),
            (Action::Release, Some(button)) if held.is_some_and(|held| held.button == button) => {
                MouseInput::Release(button.code())
            }
            (Action::Motion, button) if held.map(|held| held.button) == button => {
                MouseInput::Motion(button.map(Button::code))
            }
            (Action::WheelUp, None) => MouseInput::WheelUp,
            (Action::WheelDown, None) => MouseInput::WheelDown,
            _ => return Ok(false),
        };
        if terminal.publication_held() && !matches!(input, MouseInput::Release(_)) {
            return Ok(false);
        }
        let modifiers = (u8::from(report.alt) << 3) | (u8::from(report.ctrl) << 4);
        let Some(bytes) = terminal.encode_mouse(input, report.col, report.row, modifiers) else {
            return Ok(false);
        };
        session.submit_input(bytes)?;
        match input {
            MouseInput::Press(_)
                if profile.is_some_and(|profile| profile.tracking != MouseTracking::PressOnly) =>
            {
                self.pressed = report.button.map(|button| Pressed {
                    stamp: presented,
                    generation: report.generation,
                    button,
                });
            }
            MouseInput::Press(_) | MouseInput::Release(_) => self.pressed = None,
            _ => {}
        }
        Ok(true)
    }
}

fn generation(terminal: &Terminal) -> Option<u32> {
    u32::try_from(terminal.mouse_report_generation()).ok()
}

fn same_surface(left: Stamp, right: Stamp) -> bool {
    left.epoch == right.epoch && left.layout == right.layout && left.screen == right.screen
}
