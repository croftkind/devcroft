# Expand the sidebar to opened files

## Goal

Opening a file in the built-in editor expands all of its parent folders in the sidebar tree, including when switching tabs or reopening the current file.

## Non-goals

Change sidebar visibility, focus, scrolling, indexing rules, or tree pagination.

## Files that own this behavior

- `src/editor/native.rs`: shared file opening, file creation, sidebar expansion state, and regression tests.
- `docs/editor.md`: everyday editor behavior.

## Approach

- [x] Extract parent-folder expansion from file creation into a shared helper.
- [x] Call it after every successful open, including the current-file fast path.
- [x] Cover nested files, tab switches, reopening, and failed opens; preserve unrelated folder state.
- [x] Update editor documentation, run focused checks, and record independent review and live UI observations.

## Acceptance criteria

- A successfully opened nested file has all parent folders expanded.
- Switching to an existing tab or reopening the current file expands manually collapsed parents again.
- Failed opens do not alter expansion state.
- Unrelated folders retain their expansion state, and the sidebar mode and editor focus continue to follow existing behavior.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test --locked editor::native::tests::`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo build --locked`
- Computer-use agent: open a nested file from the finder and switch tabs after collapsing its parents.

Verification completed: the new regression failed before implementation, then all 53 native-editor tests passed with the fix. Formatting, diff whitespace checks, clippy, and the desktop build passed. Independent review found no actionable issues ([review](devcroft:artifact/art-2g68nk8a)).

The computer-use agent reproduced the original defect in the running packaged app. It launched the fresh build but could not select that standalone process through the available UI tool, so live verification of the new build remains unconfirmed. It stopped its test process and left both pre-existing app instances open.

## Risks / open questions

- Files excluded from the existing project index cannot be shown in the tree; opening them still expands their parent paths.
- Existing tree pagination remains in effect for folders with more than 100 children.
