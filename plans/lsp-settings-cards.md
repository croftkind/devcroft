# Simplify language server settings

## Goal

Make language server management easier to scan with compact cards, enable switches, and a clear hierarchy for metadata and actions.

## Non-goals

Change server discovery, installation, runtime requirements, or workspace restart behavior.

## Files that own this behavior

- `src/editor/lsp/settings.rs`: shared settings cards, enable preferences, configuration, and management actions.
- `src/settings.rs`: embeds the shared cards in Settings.
- `src/editor/native.rs`: embeds the same cards in the editor's language server dialog.
- `src/editor/lsp/install.rs`: installed executable and previous-version availability.
- `src/workspace.rs`: mounts Settings without an outer dialog scroller.

## Approach

- [x] Inspect the existing shared surface and native switch/menu components.
- [x] Replace Enable/Disable buttons with labeled, controlled switches.
- [x] Separate language titles from muted server/source metadata; keep long paths in advanced settings.
- [x] Keep installation and configuration visible; group managed maintenance controls in the server dialog.
- [x] Group advanced configuration fields with labels and spacing, keeping operation feedback visible.
- [x] Build, run focused LSP checks, obtain independent review, and verify the native layout.
- [x] Replace inline configuration with a per-server Advanced settings dialog, with Save and Cancel, validation feedback, and fresh persisted values on each opening.
- [x] Verify cancellation, validation, nested dialog behavior, and unchanged card layout.
- [x] Show switches only for available installations and remove redundant Install actions for system servers.
- [x] Replace the overflow menu with clearly labeled managed installation controls inside the server dialog.
- [x] Keep Settings sidebar and section header stationary while only section content scrolls.
- [x] Verify availability transitions, maintenance controls, and scrolling with focused regressions and native inspection.

## Acceptance criteria

- Servers with an installed executable and required runtime have a switch reflecting their persisted enabled preference. Missing servers offer installation, broken custom paths offer configuration, and missing runtimes offer setup, without an enabled switch.
- Collapsed cards expose language, server identifier, installation source/version, and primary actions without long executable paths.
- Managed servers retain update, rollback (when a previous version exists), and uninstall actions in their dialog; busy jobs prevent conflicting actions and retain cancellation. Cards have no overflow menu.
- Advanced settings opens a separate dialog and retains every existing input and save action. Cancel and dismissal discard edits; validation errors remain in the dialog.
- Cards and controls remain readable in Settings and the narrower editor dialog.
- Settings sidebar and section header stay fixed while section content scrolls; the language server panel does not add another scroll area.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test --locked editor::lsp:: -- --test-threads=1`
- `cargo test --locked settings::tests:: -- --test-threads=1`
- `cargo build --locked`
- `git diff --check`
- Delegated native visual verification of Settings, the Advanced settings dialog, and stationary sidebar/header during content scrolling.

Build, formatting, and diff checks pass. The focused LSP suite passes 36 tests, with two existing live/download smoke tests ignored. The Settings filter passes 12 tests, including three shared LSP settings tests (45 distinct focused tests total). Regressions cover executable/runtime availability, previous-version availability, blocked enable requests for missing executables, fixed sidebar bounds while content moves, and configuration validation/cancellation/save behavior. Independent review is recorded at [review](devcroft:artifact/art-gr3uh9da).

Delegated native verification confirmed all four cards fit together in the Settings content viewport. Missing JS/TS shows Install with no switch; system Rust has a switch and no Install; managed Go/Python have switches and Advanced settings without overflow menus. Go's dialog shows Check for updates and Uninstall, with Restore omitted when no previous executable exists.

The Advanced settings dialog opens above Settings and leaves the card size unchanged. Native verification confirmed labeled fields and visible Save/Cancel controls, inline validation for relative executable paths, canceled drafts cleared on reopening, and Escape dismissing only the top dialog. No server installation, update, removal, or configuration save was performed. The separate debug process was closed and the installed app restored.

Native scrolling verification confirmed that only the section body moves; the Settings title, all sidebar entries, and Editor section heading remain fixed. The same behavior passed in a narrower 1150×800 window. Switching sections after scrolling is covered by the regression.

Evidence: `/private/tmp/devcroft-lsp-logical-settings.png`, `/private/tmp/devcroft-settings-sticky-sidebar.png`, and `/private/tmp/devcroft-lsp-advanced-dialog.png`. The editor's separate 720-pixel dialog was reviewed in code but was not separately exercised on screen.

## Risks / open questions

- The shared component also renders in a 720-pixel editor dialog, so metadata must shrink and actions must wrap.
- Preserve installed-app sessions and existing server installations during UI verification; use the rebuilt debug app separately.
