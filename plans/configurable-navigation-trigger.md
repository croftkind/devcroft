# Configurable navigation trigger

## Goal

Let users record and save their own navigation-mode trigger in Settings → Keybindings, with immediate application and a reset to the platform default.

## Non-goals

Rebind navigation actions, other global shortcuts, or operating-system shortcuts. Change navigation behavior inside dialogs and palettes.

## Files that own this behavior

- `src/navigation_shortcut.rs`: shortcut capture, validation, matching, and display.
- `src/data/device.rs`: machine-local persisted preference and fallback.
- `src/settings.rs`: shortcut recorder, cancellation, errors, and reset.
- `src/workspace.rs`: keyboard interception, repeat handling, header badge, and overlay.
- `src/main.rs`: module registration.
- `docs/keyboard-reference.md`: user-facing instructions.

## Approach

- [x] Trace the current trigger, settings persistence, and navigation interceptor.
- [x] Add one validated keystroke preference, preserving Cmd+J / Ctrl+J by default.
- [x] Add an accessible recorder with cancellation, reserved-shortcut feedback, and reset.
- [x] Apply the selected shortcut immediately and keep all visible hints consistent.
- [x] Add regressions for persistence, capture, live replacement, and key-repeat handling.
- [x] Run formatting, focused tests, build, independent review, and attempt delegated native UI verification (host limitations below).

## Acceptance criteria

- Existing installations retain Cmd+J on macOS and Ctrl+J elsewhere.
- Users can record a single key or modifier chord, cancel with Escape, and reset the preference.
- Invalid or reserved palette/quit shortcuts do not replace the working trigger; save failures remain visible.
- A saved trigger survives restart; malformed stored shortcuts fall back to the default.
- The new trigger replaces the old one immediately, works in focused components, toggles once per physical press, and appears in the header and overlay.
- Dialogs and palettes retain their existing keyboard behavior.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test --locked navigation -- --test-threads=1`
- `cargo test --locked settings::tests:: -- --test-threads=1`
- `cargo test --locked data::device:: -- --test-threads=1`
- `cargo build --locked`
- `git diff --check`
- Independent reviewer and delegated native UI verification.

Formatting, diff checks, and the final build pass. The navigation filter passes
30 tests, Settings passes 13 tests, and device state passes 18 tests (59 distinct
tests because two tests also run under the navigation filter). Regressions cover
recording, cancellation by Escape and focus loss, reserved shortcuts, save
failures, reset, restart loading, malformed stored values, live replacement,
unmodified-key repeats, dialog text input, and both palette queries.

Independent review has no unresolved findings and is recorded at
[review](devcroft:artifact/art-yemgayn8). Review found and resolved fresh-key
claims inside modal dialogs, plain-trigger interception of palette searches,
and recorder focus-loss behavior.

Native verification was delegated to the computer-use agent, but no usable UI
was available. Cua Driver could not start its socket under the host sandbox and
reported missing Accessibility/Screen Recording grants. The built-in fallback
returned app inventory but timed out acquiring the debug app. No native layout
or interaction result is claimed. The device's original default navigation
preference remains unchanged.

## Risks / open questions

- Unmodified keys take priority over normal typing; the recorder explains this behavior.
- System-reserved chords may be intercepted before the app receives them.
- Keyboard layouts can alter the character produced; match the recorded key and modifiers rather than typed text.
