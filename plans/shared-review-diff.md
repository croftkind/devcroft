# Shared Review and Git diff component

## Goal

Use GPUI Kit's retained, virtualized Diff component in repository Review, as
Git Changes and History already do. Always wrap Split diffs and remove the Git
Wrap control.

## Non-goals

- Change review scopes, Git loading, comment storage or anchor relocation.
- Change the text comparison tool or merge editor.
- Upgrade or vendor GPUI Kit.

## Files that own this behavior

- `src/diff/kit_viewer.rs`: Git diff controls and shared presentation defaults.
- `src/diff/model.rs`, `src/diff/patch.rs`: bounded source lines and patch bridge.
- `src/review/mod.rs`, `src/review/feedback.rs`: Review loading, navigation,
  viewed files, annotations and comment interactions.
- `src/review/stream.rs`: file headers and logical comment-anchor rows.
- `src/review/tests.rs`, `src/backend_benchmarks.rs`: regression coverage.
- `docs/git.md`: user-facing diff behavior.

## Approach

- [x] Make wrapping a consequence of Split mode; remove the Wrap button/state.
- [x] Preserve retained lines' exact newline bytes in the shared bounded model,
  then bridge Review's snapshot to Kit files without rereading the worktree.
- [x] Retain one DiffState for all Review files. Supply Review headers,
  unavailable/truncation notices, comment threads and editors through Kit hooks.
- [x] Add Unified/Split and change navigation controls to Review. Keep viewed
  marks, collapsing, tree-to-file navigation, editor opening, comment selection,
  refresh generation checks and unchanged-load state retention.
- [x] Verify automated behavior and obtain an independent reviewer artifact.
- [ ] Complete focused native UI verification (attempted; the native UI tool
  cannot select the unbundled debug executable).

## Acceptance criteria

- Git Changes, History and Review use Kit Diff with Split wrapping automatically.
- No Git Wrap option appears; selection and source copying still work.
- Review offers rich unified/split display, inline changes, source selection,
  copy and previous/next change navigation.
- Review comments attach to the correct old/new source side and range. Drafts,
  edits, saved threads, outdated anchors and out-of-diff comments remain usable.
- Viewed/collapsed state and stale-load protection retain their existing rules.
- Binary, oversized and truncated diffs remain explicit and bounded.
- Unchanged refreshes preserve the retained diff, selection and viewport.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test --locked diff:: -- --test-threads=1`
- `cargo test --locked review:: -- --test-threads=1`
- `cargo test --locked git::view:: -- --test-threads=1`
- `cargo test --locked tools:: -- --test-threads=1`
- `cargo test --locked --test artifact_cli --test relationships_cli --test review_cli --test skill_cli -- --test-threads=1`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo build --locked`
- Focused native UI inspection of Review and Git History/Changes.

## Implementation and verification results

- Review now renders all files through one retained Kit DiffState, with its
  existing comments, viewed marks and file navigation integrated through Kit's
  headers, annotations and source-range APIs. Split mode wraps automatically in
  both Review and Git; the Git Wrap control is removed.
- Review's Unified/Split and previous/next change controls share the main
  toolbar with scope, comments and Refresh; there is no separate controls row
  above the file list. Total additions and deletions appear in the Files header,
  colored green and red respectively, alongside the file count.
- The bounded snapshot bridge preserves LF/CRLF, final-newline state and literal
  CR bytes. Shared diffing now counts LF-delimited source lines consistently
  with Git. Binary, unavailable, metadata-only and truncated files remain
  explicit without fabricated source hunks.
- Final focused checks pass: 77 Review, 46 diff, 11 Git view, 67 tools, and 8 CLI
  integration tests (209 total). Formatting, strict Clippy, debug build and
  `git diff --check` pass. An earlier full suite passed 820 tests with 7 ignored;
  its single sandbox-restricted localhost test passed on an approved rerun.
- [Independent review](devcroft:artifact/art-v6qjrwgc) reports no remaining
  actionable static findings after fixes for focus, word selection, exact CR
  handling, no-source explanations, and comment-editor reveal/scrolling.
- Native verification was delegated. After approval review became available,
  the debug process launched, but the native selector could not bind its
  unbundled executable. Final long-line rendering, bottom-of-viewport comment
  interactions and the single toolbar row remain visually unverified. All 77
  Review tests and the build passed again after moving the controls.

## Risks / open questions

- Kit owns its virtualized list and exposes scrolling to source lines/files,
  but no public viewport getter or scroll event. The old Review list's exact
  scroll-spy and pixel-position restoration cannot be transferred directly.
  Preserve unchanged loads; restore changed loads using a source selection or
  selected file. File tree selection remains a direct navigation control.
- Patch construction must preserve CRLF, final-newline state, sparse source
  coordinates and complete-hunk truncation without re-reading mutable files.
- Annotations must not duplicate editors or hide saved comments when a source
  line disappears or a file is collapsed.
