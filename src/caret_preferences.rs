//! Device-local policy shared by Kit text inputs and the built-in editor.
use std::time::Duration;

use gpui_kit::App;
use gpui_kit::component::{Theme, input::CursorBlink};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CaretPreference {
    #[default]
    Blink,
    Steady,
    Idle,
}

impl CaretPreference {
    pub(crate) const ALL: [Self; 3] = [Self::Blink, Self::Steady, Self::Idle];

    pub(crate) fn from_name(name: Option<&str>) -> Self {
        match name {
            Some("steady") => Self::Steady,
            Some("idle") => Self::Idle,
            _ => Self::Blink,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Blink => "blink",
            Self::Steady => "steady",
            Self::Idle => "idle",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Blink => "Blink",
            Self::Steady => "Steady",
            Self::Idle => "Stop after idle",
        }
    }

    fn policy(self) -> CursorBlink {
        match self {
            Self::Blink => CursorBlink::default(),
            Self::Steady => CursorBlink::steady(),
            Self::Idle => CursorBlink::blinking(Duration::from_millis(500))
                .with_idle_timeout(Duration::from_secs(5)),
        }
    }

    pub(crate) fn apply(self, cx: &mut App) {
        Theme::update(cx, |theme| theme.cursor_blink = self.policy());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_names_and_unknown_defaults_keep_valid_policies() {
        for choice in CaretPreference::ALL {
            assert_eq!(CaretPreference::from_name(Some(choice.name())), choice);
        }
        assert_eq!(CaretPreference::from_name(None), CaretPreference::Blink);
        assert_eq!(
            CaretPreference::from_name(Some("future-policy")),
            CaretPreference::Blink
        );
        assert!(CaretPreference::Steady.policy().is_steady());
        assert_eq!(
            CaretPreference::Idle.policy().idle_timeout(),
            Some(Duration::from_secs(5))
        );
    }
}
