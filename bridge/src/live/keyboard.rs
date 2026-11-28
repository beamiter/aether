use super::KeypadReport;
use jterm_engine::{
    input::{
        encode_kitty_text_press, kitty_keypad_report, kitty_modifier_report, kitty_named_report,
        kitty_text_report, Key, KeyboardLocks, KittyEventType, KittyKeyReport, KittyModifierKey,
        KittyTextKey, Modifiers, KITTY_REPORT_ALL_KEYS, KITTY_REPORT_EVENT_TYPES,
    },
    terminal::Terminal,
};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeyPhase {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEvent {
    pub phase: KeyPhase,
    pub name: String,
    pub code: String,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub super_key: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub focus_generation: u32,
    pub text: Option<String>,
    pub native_time: Option<u64>,
}

#[derive(Clone, Copy)]
pub struct NativeKeyState {
    pub base: Option<char>,
    pub locks: KeyboardLocks,
}

impl KeyEvent {
    pub fn hardware_code(&self) -> Option<u16> {
        position(&self.code).map(|(code, _)| code)
    }

    pub fn character(&self) -> Option<char> {
        let mut chars = self.name.chars();
        let ch = chars.next()?;
        (chars.next().is_none() && !ch.is_control()).then_some(ch)
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.len() > 32
            || self.code.len() > 32
            || self.name.chars().any(char::is_control)
            || !self.code.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err("invalid keyboard event".into());
        }
        if let Some(text) = &self.text {
            if self.phase == KeyPhase::Release
                || !self.character().is_some_and(|ch| text.chars().eq([ch]))
            {
                return Err("invalid associated key text".into());
            }
        }
        Ok(())
    }

    fn modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.shift,
            alt: self.alt,
            ctrl: self.ctrl,
            super_key: self.super_key,
        }
    }

    fn modifier(&self) -> Option<KittyModifierKey> {
        if matches!(
            self.name.as_str(),
            "Shift"
                | "Control"
                | "Alt"
                | "AltGraph"
                | "Meta"
                | "Super"
                | "CapsLock"
                | "NumLock"
                | "ScrollLock"
        ) {
            return modifier_for_code(&self.code);
        }
        None
    }
}

#[derive(Default)]
pub(super) struct Keyboard {
    generation: Option<(u64, u32)>,
    held: Vec<(u16, KittyKeyReport)>,
}

pub(super) struct PreparedKey {
    pub bytes: Vec<u8>,
    pub tracks_release: bool,
    hold: Option<(u16, KittyKeyReport)>,
}

impl PreparedKey {
    fn ordinary(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            tracks_release: false,
            hold: None,
        }
    }
}

impl Keyboard {
    pub fn clear(&mut self) {
        self.generation = None;
        self.held.clear();
    }

    fn retire(&mut self, code: u16) -> Option<KittyKeyReport> {
        let index = self.held.iter().position(|(held, _)| *held == code)?;
        Some(self.held.swap_remove(index).1)
    }

    pub fn admit(&mut self, prepared: PreparedKey) {
        if let Some((code, report)) = prepared.hold {
            self.held.push((code, report));
        }
    }

    pub fn prepare(
        &mut self,
        terminal: &Terminal,
        event: &KeyEvent,
        native: Option<NativeKeyState>,
    ) -> Result<PreparedKey, String> {
        event.validate()?;
        let (flags, generation) = terminal.keyboard_report_state();
        let context = (generation, event.focus_generation);
        if self.generation != Some(context) {
            self.held.clear();
            self.generation = Some(context);
        }
        let physical = event.hardware_code();
        let modifiers = event.modifiers();
        let locks = native.map_or(
            KeyboardLocks {
                caps_lock: event.caps_lock,
                num_lock: event.num_lock,
            },
            |state| state.locks,
        );
        let report_locks = if flags & KITTY_REPORT_ALL_KEYS != 0 {
            locks
        } else {
            KeyboardLocks {
                caps_lock: false,
                num_lock: locks.num_lock && KeypadReport::position_from_code(&event.code).is_some(),
            }
        };
        if event.phase == KeyPhase::Release {
            let previous = physical.and_then(|code| self.retire(code));
            let bytes = previous
                .and_then(|report| {
                    report.encode(
                        KittyEventType::Release,
                        modifiers,
                        report_locks,
                        flags,
                        None,
                    )
                })
                .unwrap_or_default();
            return Ok(PreparedKey::ordinary(bytes));
        }
        if event.phase == KeyPhase::Press {
            if let Some(code) = physical {
                self.retire(code);
            }
        }
        let base = base_character(event, native, locks.caps_lock);
        let text = event.text.as_deref();
        if let Some(report) = select_report(event, modifiers, base, flags)? {
            let track = if flags & KITTY_REPORT_EVENT_TYPES != 0 {
                physical
            } else {
                None
            };
            let (report, phase, hold) = if let Some(code) = track {
                if event.phase == KeyPhase::Repeat {
                    let Some((_, pressed)) = self.held.iter().find(|(held, _)| *held == code)
                    else {
                        return Ok(PreparedKey::ordinary(Vec::new()));
                    };
                    (
                        pressed.with_repeat_alternate(report),
                        KittyEventType::Repeat,
                        None,
                    )
                } else {
                    (report, KittyEventType::Press, Some((code, report)))
                }
            } else {
                (report, KittyEventType::Press, None)
            };
            let bytes = report
                .encode(phase, modifiers, report_locks, flags, text)
                .unwrap_or_default();
            return Ok(PreparedKey {
                bytes,
                tracks_release: track.is_some(),
                hold,
            });
        }
        if event.modifier().is_some() || modifiers.super_key {
            return Ok(PreparedKey::ordinary(Vec::new()));
        }
        if let Some(bytes) = encode_kitty_text_press(None, modifiers, report_locks, flags, text) {
            return Ok(PreparedKey::ordinary(bytes));
        }
        let bytes = if let Some(position) = KeypadReport::position_from_code(&event.code) {
            terminal.encode_key_with_keypad(
                &event.name,
                base,
                modifiers,
                position,
                locks.num_lock,
            )?
        } else if let Some(text) = text {
            terminal.encode_text(text)?
        } else if event.name == "Unidentified" {
            Vec::new()
        } else {
            terminal.encode_key_with_base(
                &event.name,
                base,
                modifiers.shift,
                modifiers.alt,
                modifiers.ctrl,
            )?
        };
        Ok(PreparedKey::ordinary(bytes))
    }
}

fn select_report(
    event: &KeyEvent,
    mods: Modifiers,
    base: Option<char>,
    flags: u16,
) -> Result<Option<KittyKeyReport>, String> {
    if let Some(modifier) = event.modifier() {
        return Ok(kitty_modifier_report(modifier, flags));
    }
    let key = if event.name == "Unidentified" || (event.code == "Numpad5" && event.name == "Clear")
    {
        Key::Unidentified
    } else {
        Terminal::key_from_name(&event.name)?
    };
    if let Some(keypad) = KeypadReport::position_from_code(&event.code) {
        return Ok(kitty_keypad_report(keypad, key, mods, flags));
    }
    if let Key::Character(actual) = key {
        let Some(base) = base else { return Ok(None) };
        return Ok(kitty_text_report(
            KittyTextKey {
                base,
                shifted: mods.shift.then_some(actual),
                base_layout: position(&event.code).and_then(|(_, base)| base),
            },
            mods,
            flags,
        ));
    }
    Ok(kitty_named_report(key, mods, flags))
}

fn base_character(
    event: &KeyEvent,
    native: Option<NativeKeyState>,
    caps_locked: bool,
) -> Option<char> {
    let actual = event.character()?;
    if let Some(base) = native.and_then(|state| state.base) {
        return Some(base);
    }
    if !event.shift && !caps_locked {
        return Some(actual);
    }
    if actual.is_alphabetic() || actual == ' ' {
        let mut lower = actual.to_lowercase();
        let ch = lower.next()?;
        return lower.next().is_none().then_some(ch);
    }
    None
}

fn modifier_for_code(code: &str) -> Option<KittyModifierKey> {
    use KittyModifierKey as Modifier;
    Some(match code {
        "ShiftLeft" => Modifier::ShiftLeft,
        "ShiftRight" => Modifier::ShiftRight,
        "ControlLeft" => Modifier::ControlLeft,
        "ControlRight" => Modifier::ControlRight,
        "AltLeft" => Modifier::AltLeft,
        "AltRight" => Modifier::AltRight,
        "MetaLeft" | "OSLeft" => Modifier::SuperLeft,
        "MetaRight" | "OSRight" => Modifier::SuperRight,
        "CapsLock" => Modifier::CapsLock,
        "NumLock" => Modifier::NumLock,
        "ScrollLock" => Modifier::ScrollLock,
        _ => return None,
    })
}

fn position(code: &str) -> Option<(u16, Option<char>)> {
    let ascii = match code.as_bytes() {
        [b'K', b'e', b'y', letter @ b'A'..=b'Z'] => [
            (24, b"QWERTYUIOP".as_slice()),
            (38, b"ASDFGHJKL"),
            (52, b"ZXCVBNM"),
        ]
        .into_iter()
        .find_map(|(start, row)| {
            row.iter()
                .position(|c| c == letter)
                .map(|offset| (start + offset as u16, letter.to_ascii_lowercase()))
        }),
        [b'D', b'i', b'g', b'i', b't', digit @ b'0'..=b'9'] => {
            Some((10 + u16::from((digit - b'0' + 9) % 10), *digit))
        }
        _ => match code {
            "Minus" => Some((20, b'-')),
            "Equal" => Some((21, b'=')),
            "BracketLeft" => Some((34, b'[')),
            "BracketRight" => Some((35, b']')),
            "Semicolon" => Some((47, b';')),
            "Quote" => Some((48, b'\'')),
            "Backquote" => Some((49, b'`')),
            "Backslash" => Some((51, b'\\')),
            "Comma" => Some((59, b',')),
            "Period" => Some((60, b'.')),
            "Slash" => Some((61, b'/')),
            "Space" => Some((65, b' ')),
            _ => None,
        },
    };
    if let Some((code, ch)) = ascii {
        return Some((code, Some(char::from(ch))));
    }
    // Linux GDK's XKB keycodes are used to find the native layout sample.
    let hardware = match code {
        "Escape" => 9,
        "Backspace" => 22,
        "Tab" => 23,
        "Enter" => 36,
        "ControlLeft" => 37,
        "ShiftLeft" => 50,
        "ShiftRight" => 62,
        "NumpadMultiply" => 63,
        "AltLeft" => 64,
        "CapsLock" => 66,
        "F1" => 67,
        "F2" => 68,
        "F3" => 69,
        "F4" => 70,
        "F5" => 71,
        "F6" => 72,
        "F7" => 73,
        "F8" => 74,
        "F9" => 75,
        "F10" => 76,
        "NumLock" => 77,
        "ScrollLock" => 78,
        "Numpad7" => 79,
        "Numpad8" => 80,
        "Numpad9" => 81,
        "NumpadSubtract" => 82,
        "Numpad4" => 83,
        "Numpad5" => 84,
        "Numpad6" => 85,
        "NumpadAdd" => 86,
        "Numpad1" => 87,
        "Numpad2" => 88,
        "Numpad3" => 89,
        "Numpad0" => 90,
        "NumpadDecimal" => 91,
        "IntlBackslash" => 94,
        "F11" => 95,
        "F12" => 96,
        "NumpadEnter" => 104,
        "ControlRight" => 105,
        "NumpadDivide" => 106,
        "AltRight" => 108,
        "Home" => 110,
        "ArrowUp" => 111,
        "PageUp" => 112,
        "ArrowLeft" => 113,
        "ArrowRight" => 114,
        "End" => 115,
        "ArrowDown" => 116,
        "PageDown" => 117,
        "Insert" => 118,
        "Delete" => 119,
        "NumpadEqual" => 125,
        "NumpadComma" => 129,
        "MetaLeft" | "OSLeft" => 133,
        "MetaRight" | "OSRight" => 134,
        _ => return None,
    };
    Some((hardware, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(phase: KeyPhase) -> KeyEvent {
        KeyEvent {
            phase,
            name: "a".into(),
            code: "KeyA".into(),
            shift: false,
            alt: false,
            ctrl: false,
            super_key: false,
            caps_lock: false,
            num_lock: false,
            focus_generation: 1,
            text: (phase != KeyPhase::Release).then(|| "a".into()),
            native_time: None,
        }
    }

    #[test]
    fn keyboard_releases_follow_admission_and_retire_on_mode_or_focus_changes() {
        let mut terminal = Terminal::new(6, 16).unwrap();
        terminal.enable_native_keyboard_reporting();
        terminal.feed(b"\x1b[=26u").unwrap();
        let mut keyboard = Keyboard::default();
        let down = event(KeyPhase::Press);
        let repeat = event(KeyPhase::Repeat);
        let mut up = event(KeyPhase::Release);
        let rejected = keyboard.prepare(&terminal, &down, None).unwrap();
        assert_eq!(rejected.bytes, b"\x1b[97;;97u");
        assert!(keyboard
            .prepare(&terminal, &up, None)
            .unwrap()
            .bytes
            .is_empty());
        assert!(keyboard
            .prepare(&terminal, &repeat, None)
            .unwrap()
            .bytes
            .is_empty());
        let accepted = keyboard.prepare(&terminal, &down, None).unwrap();
        assert!(accepted.tracks_release);
        keyboard.admit(accepted);
        assert_eq!(
            keyboard.prepare(&terminal, &repeat, None).unwrap().bytes,
            b"\x1b[97;1:2;97u"
        );
        up.name = "ф".into();
        assert_eq!(
            keyboard.prepare(&terminal, &up, None).unwrap().bytes,
            b"\x1b[97;1:3u"
        );

        let accepted = keyboard.prepare(&terminal, &down, None).unwrap();
        keyboard.admit(accepted);
        terminal.feed(b"\x1b[=0u\x1b[=26u").unwrap();
        assert!(keyboard
            .prepare(&terminal, &up, None)
            .unwrap()
            .bytes
            .is_empty());
        let accepted = keyboard.prepare(&terminal, &down, None).unwrap();
        keyboard.admit(accepted);
        up.focus_generation += 1;
        assert!(keyboard
            .prepare(&terminal, &up, None)
            .unwrap()
            .bytes
            .is_empty());
    }

    #[test]
    fn shifted_symbols_use_the_layout_sample_with_a_pc101_alternate() {
        let mut terminal = Terminal::new(6, 16).unwrap();
        terminal.enable_native_keyboard_reporting();
        terminal.feed(b"\x1b[=30u").unwrap();
        let mut keyboard = Keyboard::default();
        let mut down = event(KeyPhase::Press);
        down.name = "!".into();
        down.code = "Digit3".into();
        down.shift = true;
        down.text = Some("!".into());
        let native = NativeKeyState {
            base: Some('б'),
            locks: KeyboardLocks::default(),
        };
        assert_eq!(
            keyboard
                .prepare(&terminal, &down, Some(native))
                .unwrap()
                .bytes,
            b"\x1b[1073:33:51;2;33u"
        );
    }
}
