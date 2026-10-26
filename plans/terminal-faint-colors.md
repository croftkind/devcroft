# Preserve terminal suggestion styling

## Goal

Make suggested and placeholder text visibly dimmer than typed text in terminal-backed agent panes, allow terminal applications to discover the colors Devcroft actually paints, and keep text aligned with its grid background and cursor through blinking and resizing.

## Non-goals

Change agent input behavior, introduce a terminal theme picker, or replace the terminal renderer.

## Files that own this behavior

- `src/session.rs`: Ghostty configuration, color-query responses, and cell styles.
- `src/pane.rs`: foreground, background, cursor, and block-fill painting.
- `docs/terminal.md`: supported terminal styling.

## Approach

- [x] Trace the screenshot differences to terminal style handling.
- [x] Add regressions for ANSI faint/reset, colored backgrounds, inverse text, and default-color queries.
- [x] Preserve faint styling and configure the terminal's painted default colors explicitly.
- [x] Use GPUI's text fade after opaque color highlighting, retain packed cell backgrounds, and paint backgrounds on the terminal grid for every row.
- [x] Run focused tests, build, and independent review; publish the review as a Devcroft artifact.
- [x] Delegate native visual verification of suggestions, typed input, and input backgrounds.
- [x] Reproduce and correct the text/cursor offset caused by fractional row heights after device-pixel rounding.
- [x] Verify placeholder centering and both cursor blink phases in the rebuilt app.
- [x] Update independent review with the row-layout fix and final verification.

## Acceptance criteria

- ANSI SGR 2 dims the foreground while leaving the background intact; SGR 22 and SGR 0 restore normal intensity.
- Dim styling works with default, palette, and RGB colors, including inverse colors and block fills.
- GPUI's resolved text style retains faint opacity after applying highlights over inherited text colors.
- Erased cells retain packed RGB and palette backgrounds even when Ghostty reports no style ID.
- Background rectangles cover full terminal cells, including blank input-box areas, independently of shaped text widths.
- Cursor inversion preserves its background and the text's dim styling.
- OSC 10/11 queries report the same default colors used for rendering, so agents can choose their input background.
- Existing terminal session and pane tests continue to pass.
- Text, cursor, and background row origins remain aligned near the bottom of a tall pane at 1×, 1.5×, and 2× display scales.
- Blinking changes the cursor cell's colors without moving or clipping the placeholder glyphs.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test session::tests -- --test-threads=1`
- `cargo test pane::tests -- --test-threads=1`
- `cargo build`

Formatting, all 37 focused tests (11 session and 26 pane), the build, and `git diff --check` passed. The regressions reproduced missing OSC 10/11 replies, lost packed erase backgrounds, and opaque text after GPUI highlight resolution before the corresponding fixes. Coverage includes palette/RGB colors, ANSI intensity resets, inverse colors, bold plus faint, block fills, and cursor inversion. The existing `block v0.1.6` future Rust compatibility warning remains.

The row-layout regression reproduced an 8-pixel offset at row 40: GPUI snapped each stacked 19.8-pixel row height to 20 pixels while backgrounds and cursors used the continuous terminal grid. Each text row now uses the same absolute grid origin as its background and cursor. The rendered GPUI layout regression checks matching row origins and heights at 1×, 1.5×, and 2× display scales.

Independent review found no blocking issues: [review](devcroft:artifact/art-x4cmfnpj). The [implementation plan](devcroft:artifact/art-x7afxg8v) is also published in Devcroft.

Delegated native verification of a fresh Codex session confirmed dim placeholder text, brighter unsubmitted typed text, and a complete charcoal composer background. Blank composer pixels measured RGB `(30,30,30)` against terminal black `(0,0,0)`. Evidence: `/private/tmp/devcroft-terminal-color-placeholder.png` and `/private/tmp/devcroft-terminal-color-typed.png`. The test launch used `/usr/bin/env -u NO_COLOR` because the command tool supplies `NO_COLOR=1`, which suppresses Codex's computed background colors. The temporary input was cleared without submission; only the separate debug app and its test session were closed, preserving installed-app sessions.

Follow-up native verification in a fresh Codex session confirmed that the placeholder is centered in the charcoal input band and the first `A` remains fully legible with the cursor on and off. The two captures differ only inside the cursor cell at `(171–176, 715–726)`; the remaining placeholder glyphs do not move or clip. Evidence: `/private/tmp/devcroft-fixed-codex-loaded-a.png` (cursor on) and `/private/tmp/devcroft-fixed-codex-loaded-b.png` (cursor off). No prompt was entered or submitted. The temporary Codex and OpenCode sessions were closed, the debug app exited successfully, and the original installed-app window was restored to the foreground.

## Risks / open questions

- Ghostty's render API resolves RGB/palette colors but leaves faint opacity to the application renderer.
- The color-query regression confirmed that terminal defaults were unset and OSC 10/11 reports were absent; the fix explicitly configures the colors already painted by Devcroft.
- Preserve active sessions in the installed app during visual verification; use a separate rebuilt app if needed.
- The installed application bundle was not replaced; rebuild/relaunch it to apply the fix there.
