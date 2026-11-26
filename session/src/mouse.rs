use super::Terminal;
use jterm_engine::{frame::CellFrame, parser::Event};

pub(super) const MOUSE_MODES: [u16; 6] = [1000, 1002, 1003, 1005, 1006, 1015];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseTracking {
    Off,
    Button,
    Drag,
    Any,
}

impl MouseTracking {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Button => "button",
            Self::Drag => "drag",
            Self::Any => "any",
        }
    }
}

#[derive(Clone, Copy)]
pub struct MouseState {
    pub tracking: MouseTracking,
    pub generation: u32,
}

pub struct Presentation {
    pub frame: CellFrame,
    pub mouse: MouseState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseIntent {
    Press(MouseButton),
    Release(MouseButton),
    Motion(Option<MouseButton>),
    WheelUp,
    WheelDown,
}

pub(super) fn affects_mouse(event: &Event<'_>) -> bool {
    matches!(event,
        Event::Csi { params, private_prefix: Some(b'?'), intermediates, final_byte: b'h' | b'l', .. }
            if intermediates.is_empty() && params.iter().any(|mode| MOUSE_MODES.contains(mode)
                || matches!(mode, 47 | 1047 | 1049))
    )
}

impl Terminal {
    pub(super) fn mouse_flags(&self) -> u8 {
        MOUSE_MODES.iter().enumerate().fold(
            u8::from(self.screen.use_alt_buffer),
            |flags, (index, mode)| {
                flags | (u8::from(self.protocol.modes().contains(mode)) << (index + 1))
            },
        )
    }

    pub fn mouse_state(&self) -> MouseState {
        let modes = self.protocol.modes();
        let tracking = if modes.contains(&1003) {
            MouseTracking::Any
        } else if modes.contains(&1002) {
            MouseTracking::Drag
        } else if modes.is_mouse_enabled() {
            MouseTracking::Button
        } else {
            MouseTracking::Off
        };
        MouseState {
            tracking,
            generation: self.mouse_generation,
        }
    }

    pub fn layout(&self) -> u64 {
        self.layout
    }

    pub fn presentation(&self) -> Result<Presentation, String> {
        Ok(Presentation {
            frame: self.frame()?,
            mouse: self.mouse_state(),
        })
    }

    pub fn encode_mouse(
        &self,
        intent: MouseIntent,
        row: usize,
        col: usize,
        alt: bool,
        ctrl: bool,
    ) -> Option<Vec<u8>> {
        let modes = self.protocol.modes();
        if row >= self.screen.grid.rows() || col >= self.screen.grid.cols() {
            return None;
        }
        // Shift is the WebView's local selection override. The other physical
        // modifiers keep the shared xterm Cb bit meanings.
        let modifiers = (u8::from(alt) << 3) | (u8::from(ctrl) << 4);
        let code = match intent {
            MouseIntent::Press(button) => button.code(),
            MouseIntent::Release(button) => {
                return modes.get_mouse_release_report(button.code() | modifiers, col, row);
            }
            MouseIntent::Motion(button) if modes.should_report_mouse_motion(button.is_some()) => {
                32 | button.map_or(3, MouseButton::code)
            }
            MouseIntent::Motion(_) => return None,
            MouseIntent::WheelUp => 64,
            MouseIntent::WheelDown => 65,
        };
        modes.get_mouse_report(code | modifiers, col, row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_modes_report_physical_cells_and_use_shared_formats() {
        let mut terminal = Terminal::new(8, 128).unwrap();
        let press = MouseIntent::Press(MouseButton::Left);
        let release = MouseIntent::Release(MouseButton::Left);
        let drag = MouseIntent::Motion(Some(MouseButton::Left));
        let hover = MouseIntent::Motion(None);
        assert!(terminal.encode_mouse(press, 0, 0, false, false).is_none());

        terminal.feed(b"\x1b[?1000;1006h").unwrap();
        assert_eq!(terminal.mouse_state().tracking, MouseTracking::Button);
        assert_eq!(
            terminal.encode_mouse(press, 3, 5, true, true).unwrap(),
            b"\x1b[<24;6;4M",
        );
        assert_eq!(
            terminal.encode_mouse(release, 3, 5, true, false).unwrap(),
            b"\x1b[<8;6;4m",
        );
        assert!(terminal.encode_mouse(drag, 0, 0, false, false).is_none());
        assert!(terminal.encode_mouse(press, 8, 0, false, false).is_none());
        assert_eq!(
            terminal
                .encode_mouse(MouseIntent::WheelDown, 0, 0, false, true)
                .unwrap(),
            b"\x1b[<81;1;1M",
        );

        terminal.feed(b"\x1b[?1000l\x1b[?1002h").unwrap();
        assert_eq!(terminal.mouse_state().tracking, MouseTracking::Drag);
        assert_eq!(
            terminal.encode_mouse(drag, 0, 0, false, false).unwrap(),
            b"\x1b[<32;1;1M",
        );
        assert!(terminal.encode_mouse(hover, 0, 0, false, false).is_none());

        terminal.feed(b"\x1b[?1002;1006l\x1b[?1003;1005h").unwrap();
        assert_eq!(terminal.mouse_state().tracking, MouseTracking::Any);
        assert_eq!(
            terminal.encode_mouse(hover, 0, 124, false, false).unwrap(),
            b"\x1b[MC\xc2\x9d!",
        );
        terminal.feed(b"\x1b[?1005l\x1b[?1015h").unwrap();
        assert_eq!(
            terminal.encode_mouse(release, 0, 0, true, false).unwrap(),
            b"\x1b[43;1;1M"
        );
        terminal.feed(b"\x1b[?1015l").unwrap();
        assert_eq!(
            terminal.encode_mouse(release, 0, 0, true, false).unwrap(),
            b"\x1b[M+!!"
        );
        assert_eq!(terminal.unsupported_count(), 0);
    }

    #[test]
    fn mouse_generation_advances_for_mode_and_screen_round_trips() {
        let mut terminal = Terminal::new(2, 4).unwrap();
        let original = terminal.mouse_state().generation;
        terminal.feed(b"\x1b[?1000h").unwrap();
        let enabled = terminal.mouse_state().generation;
        assert!(enabled > original);
        terminal.feed(b"\x1b[?1000hX").unwrap();
        assert_eq!(terminal.mouse_state().generation, enabled);
        terminal.feed(b"\x1b[?1000l\x1b[?1000h").unwrap();
        assert_eq!(terminal.mouse_state().generation, enabled + 2);
        terminal.feed(b"\x1b[?1049h\x1b[?1049l").unwrap();
        assert_eq!(terminal.mouse_state().generation, enabled + 4);
        assert_eq!(terminal.mouse_state().tracking, MouseTracking::Button);
    }
}
