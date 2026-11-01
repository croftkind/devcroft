# Markdown TOC scroll-spy

## Goal

Keep the active table-of-contents entry aligned with the visible section in Markdown previews and Resources artifacts.

## Non-goals

Change Markdown parsing, document styling, heading hierarchy, or outline layout.

## Files that own this behavior

- `src/preview.rs`: shared reader, list position, active heading, and outline events.
- `src/artifacts.rs`: Resources outline subscribes to the reader's active-heading events.
- `src/artifact_browser_tests.rs`: Resources integration coverage where needed.

## Approach

- [x] Reproduce the stale outline in the native app and inspect the shared list integration.
- [x] Read the current list position when rendering instead of relying on the wheel-only scroll callback and its previous visible range.
- [x] Use the actual document-end state when selecting the final heading.
- [x] Emit active-heading changes for TOC navigation as well as scrolling.
- [x] Cover wheel, scrollbar, keyboard scrolling, and event delivery in regression tests for both reader modes.
- [x] Run the required checks, obtain a reviewer review, and verify both native views.

## Acceptance criteria

Wheel, keyboard navigation, scrollbar movement, and TOC clicks keep the active heading current in both reader modes. A final heading visible near the viewport bottom does not become active before its section reaches the top, except at the actual document end. Resources receives the same active selection as the preview. Content updates preserve working scroll-spy behavior.

## Verification commands

- `cargo test --locked preview::tests`
- `cargo test --locked artifacts::tests`
- `mise run check`
- `cargo test --locked`
- `cargo build --locked`
- Native inspection of Markdown preview and Resources scrolling and TOC navigation.

## Verification results

- Formatting, compilation, and Clippy passed through `mise run check`.
- The full test suite passed: 859 tests passed and 7 intentionally ignored. The existing localhost-listener test required running outside the sandbox.
- The debug build passed.
- Regression tests cover both reader modes, wheel scrolling, keyboard movement, scrollbar offsets, TOC events, content shortening, and resize clamping.
- Independent review found no actionable issues: `devcroft:artifact/art-9erdtcux`.
- Native verification passed against the rebuilt debug app: README preview and a Resources artifact update the active entry on wheel scrolling and TOC clicks; Resources also selects the correct entries at the document start and end. A native scrollbar drag attempt was inconclusive, so scrollbar movement is verified through the automated offset regression.

## Risks / open questions

The virtualized list settles pending scroll requests during layout; verify clamping at document boundaries and avoid reading list state inside its mutable scroll callback. GPUI Fast may reuse rendering layers, so native scrolling must be checked as well as headless tests.
