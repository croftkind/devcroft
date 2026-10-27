//! The device-local navigation trigger: one key, optionally with modifiers.

use gpui_kit::{Keystroke, OperatingSystem};

use crate::command_palette::{is_quit_shortcut, palette_mode_for_shortcut};

const HOST_OS: OperatingSystem = if cfg!(target_os = "macos") {
    OperatingSystem::Mac
} else if cfg!(target_os = "windows") {
    OperatingSystem::Windows
} else {
    OperatingSystem::Linux
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavigationShortcut(Keystroke);

impl Default for NavigationShortcut {
    fn default() -> Self {
        Self(
            Keystroke::parse(if cfg!(target_os = "macos") {
                "cmd-j"
            } else {
                "ctrl-j"
            })
            .expect("default navigation shortcut"),
        )
    }
}

impl NavigationShortcut {
    pub(crate) fn parse(value: &str) -> Result<Self, &'static str> {
        let key = Keystroke::parse_with_os(value, HOST_OS)
            .map_err(|_| "Choose a single key or key combination.")?;
        Self::from_keystroke(&key)
    }

    pub(crate) fn from_keystroke(key: &Keystroke) -> Result<Self, &'static str> {
        let mut key = key.clone();
        key.key.make_ascii_lowercase();
        key.key_char = None;
        let name = key.key.as_str();
        let function_key = name
            .strip_prefix('f')
            .and_then(|number| number.parse::<u8>().ok())
            .is_some_and(|number| (1..=35).contains(&number) && name == format!("f{number}"));
        let supported = name.chars().count() == 1
            && !name.chars().any(|ch| ch.is_control() || ch.is_whitespace())
            || function_key
            || matches!(
                name,
                "space"
                    | "tab"
                    | "enter"
                    | "backspace"
                    | "delete"
                    | "up"
                    | "down"
                    | "left"
                    | "right"
                    | "home"
                    | "end"
                    | "pageup"
                    | "pagedown"
                    | "insert"
            );
        if !supported {
            return Err("Press a key, optionally with modifiers. Escape cancels recording.");
        }
        let m = key.modifiers;
        if palette_mode_for_shortcut(name, m.platform, m.control, m.alt).is_some() {
            return Err("That shortcut opens a palette. Choose another key combination.");
        }
        if is_quit_shortcut(name, m.platform, m.control, m.shift, m.alt) {
            return Err("That shortcut quits Devcroft. Choose another key combination.");
        }
        Ok(Self(key))
    }

    pub(crate) fn matches(&self, key: &Keystroke) -> bool {
        self.0.key.eq_ignore_ascii_case(&key.key) && self.0.modifiers == key.modifiers
    }

    pub(crate) fn stored_value(&self) -> String {
        self.0.unparse(HOST_OS)
    }

    pub(crate) fn label(&self) -> String {
        // GPUI's display omits Fn and leaves named keys lowercase. Keep the
        // same readable label in Settings, the header, and the navigation HUD.
        let mut parts = Vec::new();
        let m = self.0.modifiers;
        if m.function {
            parts.push("Fn");
        }
        if m.control {
            parts.push("Ctrl");
        }
        if m.alt {
            parts.push(if cfg!(target_os = "macos") {
                "⌥"
            } else {
                "Alt"
            });
        }
        if m.shift {
            parts.push(if cfg!(target_os = "macos") {
                "⇧"
            } else {
                "Shift"
            });
        }
        if m.platform {
            parts.push(if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Super"
            });
        }
        let key = self.0.key.to_uppercase();
        parts.push(&key);
        parts.join(if cfg!(target_os = "macos") { "" } else { "+" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_shortcuts_round_trip_and_match_keys_instead_of_typed_text() {
        for value in [
            "secondary-j",
            "ctrl-alt-n",
            "f8",
            "space",
            "\\",
            "shift-tab",
            "fn-f12",
            "ctrl--",
        ] {
            let shortcut = NavigationShortcut::parse(value).unwrap();
            assert_eq!(
                NavigationShortcut::parse(&shortcut.stored_value()).unwrap(),
                shortcut
            );
            let mut event = Keystroke::parse_with_os(value, HOST_OS).unwrap();
            event.key_char = Some("different text".to_owned());
            assert!(shortcut.matches(&event));
            event.modifiers.alt = !event.modifiers.alt;
            assert!(!shortcut.matches(&event));
        }
        assert_eq!(
            NavigationShortcut::default(),
            NavigationShortcut::parse("secondary-j").unwrap()
        );
    }

    #[test]
    fn navigation_shortcuts_reject_reserved_and_invalid_keys() {
        for value in [
            "",
            " ",
            "f01",
            "f0008",
            "ctrl",
            "shift",
            "fn",
            "escape",
            "unknown-key",
            "nonsense",
            "secondary-k",
            "secondary-shift-p",
            "secondary-q",
        ] {
            assert!(NavigationShortcut::parse(value).is_err(), "{value}");
        }
    }
}
