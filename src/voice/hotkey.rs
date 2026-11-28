//! The shortcut that starts Voice play, and the tracker that tells a press from
//! a release. The tracker is plain logic so it can be tested; the Windows hook
//! that feeds it lives in `hook.rs`.

// The Windows keyboard hook and tones are the only users of some of this.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// The modifier bits the tracker compares.
pub(crate) const CTRL: u8 = 1;
pub(crate) const ALT: u8 = 2;
pub(crate) const SHIFT: u8 = 4;
pub(crate) const WIN: u8 = 8;

/// Keys that have a name that is not just their character.
const NAMED_KEYS: &[(&str, &str, u32)] = &[
    ("space", "Space", 0x20),
    ("tab", "Tab", 0x09),
    ("enter", "Enter", 0x0D),
    ("backspace", "Backspace", 0x08),
    ("insert", "Insert", 0x2D),
    ("delete", "Delete", 0x2E),
    ("home", "Home", 0x24),
    ("end", "End", 0x23),
    ("pageup", "Page Up", 0x21),
    ("pagedown", "Page Down", 0x22),
    ("left", "Left", 0x25),
    ("up", "Up", 0x26),
    ("right", "Right", 0x27),
    ("down", "Down", 0x28),
    ("`", "`", 0xC0),
    ("-", "-", 0xBD),
    ("=", "=", 0xBB),
    ("[", "[", 0xDB),
    ("]", "]", 0xDD),
    ("\\", "\\", 0xDC),
    (";", ";", 0xBA),
    ("'", "'", 0xDE),
    (",", ",", 0xBC),
    (".", ".", 0xBE),
    ("/", "/", 0xBF),
];

/// The virtual-key code and the label for a key's name, if Voice play can use
/// it. Names are the ones the interface reports: `a`, `7`, `f9`, `space`.
fn key_info(name: &str) -> Option<(String, u32)> {
    if let Some(&(_, label, vk)) = NAMED_KEYS.iter().find(|(key, _, _)| *key == name) {
        return Some((label.to_owned(), vk));
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        if first.is_ascii_lowercase() {
            return Some((first.to_ascii_uppercase().to_string(), first as u32 - 32));
        }
        if first.is_ascii_digit() {
            return Some((first.to_string(), first as u32));
        }
        return None;
    }
    let number: u32 = name.strip_prefix('f')?.parse().ok()?;
    (1..=24)
        .contains(&number)
        .then(|| (format!("F{number}"), 0x6F + number))
}

/// A key and the modifiers held with it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// The key's name, as `key_info` knows it.
    pub key: String,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: "space".into(),
        }
    }
}

impl Hotkey {
    /// Builds a shortcut from what the interface saw pressed, or `None` when
    /// it could not work as one.
    pub(crate) fn from_keys(
        ctrl: bool,
        alt: bool,
        shift: bool,
        win: bool,
        key: &str,
    ) -> Option<Self> {
        let hotkey = Self {
            ctrl,
            alt,
            shift,
            win,
            key: key.to_owned(),
        };
        hotkey.is_usable().then_some(hotkey)
    }

    /// Whether the shortcut can be listened for without taking over typing.
    /// A letter, digit or other typing key needs Ctrl, Alt or Windows held
    /// with it (Shift alone would swallow capital letters). Function keys can
    /// stand alone.
    pub(crate) fn is_usable(&self) -> bool {
        if key_info(&self.key).is_none() {
            return false;
        }
        let function_key = self.key.starts_with('f') && self.key.len() > 1;
        function_key || self.ctrl || self.alt || self.win
    }

    pub(crate) fn vk(&self) -> Option<u32> {
        key_info(&self.key).map(|(_, vk)| vk)
    }

    pub(crate) fn modifiers(&self) -> u8 {
        [
            (self.ctrl, CTRL),
            (self.alt, ALT),
            (self.shift, SHIFT),
            (self.win, WIN),
        ]
        .into_iter()
        .filter(|(held, _)| *held)
        .fold(0, |bits, (_, bit)| bits | bit)
    }

    /// What is saved: `ctrl+alt+space`.
    pub(crate) fn encode(&self) -> String {
        let mut parts = Vec::new();
        for (held, name) in [
            (self.ctrl, "ctrl"),
            (self.alt, "alt"),
            (self.shift, "shift"),
            (self.win, "win"),
        ] {
            if held {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// Reads what `encode` wrote. Anything else is no shortcut.
    pub(crate) fn decode(text: &str) -> Option<Self> {
        let mut hotkey = Self {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            key: String::new(),
        };
        let mut parts: Vec<&str> = text.split('+').collect();
        let key = parts.pop()?;
        for part in parts {
            match part {
                "ctrl" => hotkey.ctrl = true,
                "alt" => hotkey.alt = true,
                "shift" => hotkey.shift = true,
                "win" => hotkey.win = true,
                _ => return None,
            }
        }
        hotkey.key = key.to_owned();
        hotkey.is_usable().then_some(hotkey)
    }

    /// How it reads in Settings: `Ctrl + Alt + Space`.
    pub(crate) fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (held, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.win, "Win"),
        ] {
            if held {
                parts.push(name.to_owned());
            }
        }
        parts.push(key_info(&self.key).map_or_else(|| self.key.clone(), |(label, _)| label));
        parts.join(" + ")
    }
}

/// The modifier a key code is, if it is one.
pub(crate) fn modifier_bit(vk: u32) -> Option<u8> {
    match vk {
        0x11 | 0xA2 | 0xA3 => Some(CTRL),
        0x12 | 0xA4 | 0xA5 => Some(ALT),
        0x10 | 0xA0 | 0xA1 => Some(SHIFT),
        0x5B | 0x5C => Some(WIN),
        _ => None,
    }
}

/// What the shortcut did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HotkeyEvent {
    Pressed,
    Released,
}

/// Follows keyboard events and reports when the shortcut goes down and up.
pub(crate) struct Tracker {
    vk: u32,
    modifiers: u8,
    active: bool,
}

/// What the tracker made of one keyboard event.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Outcome {
    pub event: Option<HotkeyEvent>,
    /// The key belongs to the shortcut, so other apps should not see it.
    pub swallow: bool,
}

impl Tracker {
    pub(crate) fn new(hotkey: &Hotkey) -> Option<Self> {
        Some(Self {
            vk: hotkey.vk()?,
            modifiers: hotkey.modifiers(),
            active: false,
        })
    }

    /// One keyboard event: the key code, whether it went down, and which
    /// modifiers were held before it.
    pub(crate) fn key(&mut self, vk: u32, down: bool, held_before: u8) -> Outcome {
        let held = match modifier_bit(vk) {
            Some(bit) if down => held_before | bit,
            Some(bit) => held_before & !bit,
            None => held_before,
        };
        if vk == self.vk {
            if down {
                if self.active {
                    return Outcome {
                        event: None,
                        swallow: true,
                    };
                }
                if held & self.modifiers == self.modifiers {
                    self.active = true;
                    return Outcome {
                        event: Some(HotkeyEvent::Pressed),
                        swallow: true,
                    };
                }
            } else if self.active {
                self.active = false;
                return Outcome {
                    event: Some(HotkeyEvent::Released),
                    swallow: true,
                };
            }
            return Outcome {
                event: None,
                swallow: false,
            };
        }
        // Letting go of a modifier ends the shortcut even if the key is still
        // down, so a stuck listening state is not possible.
        if self.active && held & self.modifiers != self.modifiers {
            self.active = false;
            return Outcome {
                event: Some(HotkeyEvent::Released),
                swallow: false,
            };
        }
        Outcome {
            event: None,
            swallow: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker() -> Tracker {
        Tracker::new(&Hotkey::default()).unwrap()
    }

    #[test]
    fn the_default_shortcut_round_trips_and_reads_well() {
        let hotkey = Hotkey::default();
        assert_eq!(hotkey.encode(), "ctrl+alt+space");
        assert_eq!(Hotkey::decode("ctrl+alt+space"), Some(hotkey.clone()));
        assert_eq!(hotkey.label(), "Ctrl + Alt + Space");
        assert_eq!(hotkey.vk(), Some(0x20));
        assert_eq!(hotkey.modifiers(), CTRL | ALT);
    }

    #[test]
    fn keys_map_to_their_codes() {
        for (name, vk, label) in [
            ("a", 0x41, "A"),
            ("z", 0x5A, "Z"),
            ("0", 0x30, "0"),
            ("f1", 0x70, "F1"),
            ("f9", 0x78, "F9"),
            ("f24", 0x87, "F24"),
            ("`", 0xC0, "`"),
            ("pageup", 0x21, "Page Up"),
        ] {
            let (found, code) = key_info(name).expect(name);
            assert_eq!((code, found.as_str()), (vk, label), "{name}");
        }
        for name in ["", "f0", "f25", "ab", "escape", "A"] {
            assert!(key_info(name).is_none(), "{name}");
        }
    }

    #[test]
    fn typing_keys_need_a_real_modifier_but_function_keys_stand_alone() {
        assert!(Hotkey::from_keys(false, false, false, false, "f9").is_some());
        assert!(Hotkey::from_keys(false, false, false, false, "a").is_none());
        assert!(Hotkey::from_keys(false, false, true, false, "a").is_none());
        assert!(Hotkey::from_keys(true, false, false, false, "a").is_some());
        assert!(Hotkey::from_keys(false, true, false, false, "space").is_some());
        assert!(Hotkey::from_keys(false, false, false, true, "v").is_some());
        assert!(Hotkey::from_keys(true, true, false, false, "escape").is_none());
    }

    #[test]
    fn saved_text_that_is_not_a_shortcut_is_ignored() {
        for text in [
            "",
            "space",
            "ctrl+",
            "ctrl+nope",
            "hyper+space",
            "ctrl+alt+a+b",
        ] {
            assert_eq!(Hotkey::decode(text), None, "{text:?}");
        }
        assert_eq!(
            Hotkey::decode("ctrl+shift+f9"),
            Hotkey::from_keys(true, false, true, false, "f9")
        );
    }

    #[test]
    fn holding_the_shortcut_presses_once_and_letting_go_releases_once() {
        let mut tracker = tracker();
        // Modifiers on their own do nothing.
        assert_eq!(tracker.key(0xA2, true, 0).event, None);
        assert_eq!(tracker.key(0xA4, true, CTRL).event, None);
        let press = tracker.key(0x20, true, CTRL | ALT);
        assert_eq!(press.event, Some(HotkeyEvent::Pressed));
        assert!(press.swallow);
        // Holding sends repeats, which neither fire nor reach other apps.
        let repeat = tracker.key(0x20, true, CTRL | ALT);
        assert_eq!(repeat.event, None);
        assert!(repeat.swallow);
        let release = tracker.key(0x20, false, CTRL | ALT);
        assert_eq!(release.event, Some(HotkeyEvent::Released));
        assert!(release.swallow);
        // The key does nothing once released.
        assert_eq!(tracker.key(0x20, false, CTRL | ALT).event, None);
    }

    #[test]
    fn the_key_alone_or_with_the_wrong_modifiers_is_left_alone() {
        let mut tracker = tracker();
        for held in [0, CTRL, ALT, SHIFT, CTRL | SHIFT] {
            let outcome = tracker.key(0x20, true, held);
            assert_eq!(outcome.event, None, "held {held}");
            assert!(!outcome.swallow, "held {held}");
        }
        // The key-up of a press that never started is not ours either.
        assert!(!tracker.key(0x20, false, 0).swallow);
    }

    #[test]
    fn extra_modifiers_do_not_stop_the_shortcut() {
        let mut tracker = tracker();
        let press = tracker.key(0x20, true, CTRL | ALT | SHIFT);
        assert_eq!(press.event, Some(HotkeyEvent::Pressed));
    }

    #[test]
    fn letting_go_of_a_modifier_first_still_ends_listening() {
        let mut tracker = tracker();
        assert_eq!(
            tracker.key(0x20, true, CTRL | ALT).event,
            Some(HotkeyEvent::Pressed)
        );
        // Alt (left) comes up while the key is still down.
        let outcome = tracker.key(0xA4, false, CTRL | ALT);
        assert_eq!(outcome.event, Some(HotkeyEvent::Released));
        assert!(!outcome.swallow, "the modifier's own event passes through");
        // The key's later release is not reported again.
        assert_eq!(tracker.key(0x20, false, CTRL).event, None);
    }

    #[test]
    fn a_function_key_works_alone() {
        let hotkey = Hotkey::from_keys(false, false, false, false, "f9").unwrap();
        let mut tracker = Tracker::new(&hotkey).unwrap();
        assert_eq!(tracker.key(0x78, true, 0).event, Some(HotkeyEvent::Pressed));
        assert_eq!(
            tracker.key(0x78, false, 0).event,
            Some(HotkeyEvent::Released)
        );
    }
}
