//! Match editable WebKit key events to the native layout that produced them.
use std::collections::VecDeque;
use tauri_terminal_frame_bridge::live::{KeyEvent, KeyPhase, NativeKeyState};

const SAMPLE_LIMIT: usize = 64;
const MATCH_WINDOW_MS: u64 = 350;

#[derive(Default)]
pub(crate) struct Samples {
    recent: VecDeque<Sample>,
}

struct Sample {
    hardware: u16,
    down: bool,
    character: Option<char>,
    shift: bool,
    at_ms: u64,
    state: NativeKeyState,
}

impl Samples {
    fn remember(&mut self, sample: Sample) {
        if self.recent.len() == SAMPLE_LIMIT {
            self.recent.pop_front();
        }
        self.recent.push_back(sample);
    }

    pub(crate) fn take(&mut self, event: &KeyEvent) -> Option<NativeKeyState> {
        let at_ms = event.native_time?;
        let hardware = event.hardware_code()?;
        let down = event.phase != KeyPhase::Release;
        let character = event.character();
        let best = self.recent.iter().position(|sample| {
            sample.hardware == hardware
                && sample.down == down
                && sample.character == character
                && (character.is_none() || sample.shift == event.shift)
                && at_ms.abs_diff(sample.at_ms) <= MATCH_WINDOW_MS
        })?;
        self.recent.remove(best).map(|sample| sample.state)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn install(
    view: &webkit2gtk::WebView,
    samples: std::sync::Arc<std::sync::Mutex<Samples>>,
) {
    use gtk::prelude::WidgetExt;
    let Some(keymap) = gtk::gdk::Keymap::for_display(&view.display()) else {
        return;
    };
    let mut connections = [true, false].into_iter().map(|down| {
        let samples = samples.clone();
        let keymap = keymap.clone();
        move |_: &webkit2gtk::WebView, event: &gtk::gdk::EventKey| {
            if let Some(sample) = snapshot(event, &keymap, down) {
                if let Ok(mut samples) = samples.lock() {
                    samples.remember(sample);
                }
            }
            gtk::glib::Propagation::Proceed
        }
    });
    view.connect_key_press_event(connections.next().unwrap());
    view.connect_key_release_event(connections.next().unwrap());
}

#[cfg(target_os = "linux")]
fn snapshot(event: &gtk::gdk::EventKey, keymap: &gtk::gdk::Keymap, down: bool) -> Option<Sample> {
    use gtk::gdk::{keys::Key, ModifierType};
    use jterm_engine::input::KeyboardLocks;
    use std::time::{SystemTime, UNIX_EPOCH};

    let at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()?;
    let hardware = event.hardware_keycode();
    let modifiers = event.state();
    let character = event.keyval().to_unicode().filter(|ch| !ch.is_control());
    // Ordinary printable input already carries the layout symbol. Translate
    // through GDK only when Shift or Caps Lock can change the primary key.
    let base = character.filter(|_| down).and_then(|ch| {
        if modifiers.intersects(ModifierType::SHIFT_MASK | ModifierType::LOCK_MASK) {
            keymap
                .translate_keyboard_state(
                    u32::from(hardware),
                    ModifierType::empty(),
                    i32::from(event.group()),
                )
                .and_then(|(key, _, _, _)| Key::from(key).to_unicode())
                .filter(|ch| !ch.is_control())
        } else {
            Some(ch)
        }
    });
    Some(Sample {
        hardware,
        down,
        character,
        shift: modifiers.contains(ModifierType::SHIFT_MASK),
        at_ms,
        state: NativeKeyState {
            base,
            locks: KeyboardLocks {
                caps_lock: modifiers.contains(ModifierType::LOCK_MASK),
                num_lock: modifiers.contains(ModifierType::MOD2_MASK),
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jterm_engine::input::KeyboardLocks;

    #[test]
    fn native_keyboard_samples_match_phase_code_character_modifiers_and_time_in_order() {
        let mut samples = Samples::default();
        for (hardware, down, character, shift, at_ms, base) in [
            (11, true, Some('#'), true, 500, '2'),
            (12, true, Some('#'), true, 5, '2'),
            (12, false, Some('#'), true, 499, '2'),
            (12, true, Some('!'), true, 498, '2'),
            (12, true, Some('#'), false, 500, '2'),
            (12, true, Some('#'), true, 480, '3'),
            (12, true, Some('#'), true, 502, '2'),
        ] {
            samples.remember(Sample {
                hardware,
                down,
                character,
                shift,
                at_ms,
                state: NativeKeyState {
                    base: Some(base),
                    locks: KeyboardLocks::default(),
                },
            });
        }
        let event = KeyEvent {
            phase: KeyPhase::Press,
            name: "#".into(),
            code: "Digit3".into(),
            shift: true,
            alt: false,
            ctrl: false,
            super_key: false,
            caps_lock: false,
            num_lock: false,
            focus_generation: 0,
            text: Some("#".into()),
            native_time: Some(500),
        };
        assert_eq!(samples.take(&event).unwrap().base, Some('3'));
        assert_eq!(samples.take(&event).unwrap().base, Some('2'));
        assert!(samples.take(&event).is_none());
    }
}
