use crate::protocol_v4::Counter;
use jterm_engine::{frame::Stamp, mouse::MouseInput, terminal::Terminal};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Tracking {
    Button,
    Drag,
    Any,
}

#[derive(PartialEq, Eq)]
struct Profile {
    tracking: Tracking,
    encoding: Vec<u8>,
}

impl Profile {
    fn read(terminal: &Terminal) -> Option<Self> {
        // An extended button distinguishes UTF-8 from legacy bytes even at (0, 0).
        // These probes only encode bytes and never send input to the PTY.
        let encoding = terminal.encode_mouse(MouseInput::Press(96), 0, 0, 0)?;
        let tracking = if terminal
            .encode_mouse(MouseInput::Motion(None), 0, 0, 0)
            .is_some()
        {
            Tracking::Any
        } else if terminal
            .encode_mouse(MouseInput::Motion(Some(0)), 0, 0, 0)
            .is_some()
        {
            Tracking::Drag
        } else {
            Tracking::Button
        };
        Some(Self { tracking, encoding })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MouseView {
    layout: Counter,
    revision: Counter,
    generation: u32,
    tracking: Tracking,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Press,
    Release,
    Motion,
    WheelUp,
    WheelDown,
}

#[derive(serde::Deserialize)]
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
    profile: Option<Profile>,
    generation: u32,
    pressed: Option<(Stamp, Button)>,
}

impl Mouse {
    pub(super) fn observe(&mut self, terminal: &Terminal) {
        let profile = Profile::read(terminal);
        if profile != self.profile {
            self.profile = profile;
            self.generation = self.generation.saturating_add(1);
            self.pressed = None;
        }
        if self.pressed.is_some_and(|(stamp, _)| {
            let current = terminal.frame().ok().map(|frame| frame.stamp());
            !current.is_some_and(|current| same_surface(stamp, current))
        }) {
            self.pressed = None;
        }
    }

    pub(super) fn retire(&mut self) {
        self.pressed = None;
    }

    pub(super) fn view(&self, terminal: &Terminal, presented: Stamp) -> Option<MouseView> {
        let tracking = self.profile.as_ref()?.tracking;
        let current = terminal.frame().ok()?.stamp();
        if presented != current || terminal.publication_held() {
            return None;
        }
        Some(MouseView {
            layout: Counter::new(presented.layout)?,
            revision: Counter::new(presented.revision)?,
            generation: self.generation,
            tracking,
        })
    }

    pub(super) fn matches(&self, presented: Stamp, report: &MouseReport) -> bool {
        self.generation == report.generation
            && report.revision.get() == presented.revision
            && report.layout.get() == presented.layout
    }

    pub(super) fn send(
        &mut self,
        session: &mut jterm_headless_session_spike::Session,
        presented: Stamp,
        report: MouseReport,
    ) -> Result<bool, String> {
        if self.profile.is_none() {
            return Ok(false);
        }
        let terminal = session.terminal();
        let current = terminal.frame()?.stamp();
        if !same_surface(presented, current) || terminal.publication_held() {
            return Ok(false);
        }
        let held = self
            .pressed
            .filter(|(stamp, _)| same_surface(*stamp, presented));
        let input = match (report.kind, report.button) {
            (Action::Press, Some(button)) if held.is_none() => MouseInput::Press(button.code()),
            (Action::Release, Some(button)) if held.is_some_and(|(_, held)| held == button) => {
                MouseInput::Release(button.code())
            }
            (Action::Motion, button) if held.map(|(_, held)| held) == button => {
                MouseInput::Motion(button.map(Button::code))
            }
            (Action::WheelUp, None) => MouseInput::WheelUp,
            (Action::WheelDown, None) => MouseInput::WheelDown,
            _ => return Ok(false),
        };
        let modifiers = (u8::from(report.alt) << 3) | (u8::from(report.ctrl) << 4);
        let Some(bytes) = terminal.encode_mouse(input, report.col, report.row, modifiers) else {
            return Ok(false);
        };
        session.submit_input(bytes)?;
        match input {
            MouseInput::Press(_) => self.pressed = report.button.map(|button| (presented, button)),
            MouseInput::Release(_) => self.pressed = None,
            _ => {}
        }
        Ok(true)
    }
}

fn same_surface(left: Stamp, right: Stamp) -> bool {
    left.epoch == right.epoch && left.layout == right.layout && left.screen == right.screen
}
