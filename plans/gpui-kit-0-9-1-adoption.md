# GPUI Kit 0.9.1 adoption

## Goal

Upgrade Devcroft from GPUI Kit 0.9.0 to 0.9.1, collect its input, Markdown,
scrolling and rendering fixes, and adopt clickable Markdown checklists and
styled callouts in the existing readers. Keep GPUI Fast as the default backend.

Started: 2026-11-11. Completed: 2026-11-13. Status: implemented and reviewed;
native verification limits are recorded below.

## Non-goals

- Replace resource comment controls, durable anchors or the Review patch bridge.
- Add draggable grids, table pinning, GPUI Shell or a new settings system.
- Change terminal behavior or claim performance gains without measurements.
- Autosave editor files when a checklist is clicked; use the existing draft/save flow.

## Files that own this behavior

| Files | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `README.md` | Kit/WebView alignment, backend snapshots, TLS lockfile correction and documented version |
| `src/preview.rs`, `src/preview/resource_blocks.rs` | Shared callouts, checkbox events and exact source offset mapping |
| `src/artifacts.rs`, `src/artifacts/comments.rs` | Revision-checked resource content updates and comment refresh |
| `src/editor/native.rs` | Apply checkbox edits to the live editor buffer and existing save/LSP/draft lifecycle |
| `src/backend_benchmarks.rs` | Existing opt-in CPU reader/review measurements |

## Approach

- [x] Upgrade Kit normal/dev dependencies and the exact macOS WebView pin together.
  Resolve Fast 0.1.14 and Pre 0.3.12; explicitly refresh `rustls-webpki` to at
  least 0.103.16 because the upstream TLS fix was lockfile-only.
- [x] Register restrained Note, Tip, Important, Warning and Caution callout
  surfaces in the shared reader. Preserve source, find, copy and comment behavior.
- [x] Emit exact source edits for task checkboxes only in editable hosts.
  Map resource-block and metadata display offsets back to original source;
  validate the expected source before mutating to reject stale events.
- [x] Save resource toggles with the existing artifact revision check and refresh
  anchors/comments. Apply editor toggles through its normal edit pipeline so
  drafts, undo, LSP synchronization and explicit save remain consistent.
- [x] Run formatting, strict Clippy and tests for both backends. Delegate focused
  native verification and a non-trivial-diff review; publish the review artifact.

## Acceptance criteria

- [x] Both backend configurations compile and pass the existing automated checks.
- [x] Nested and Unicode task checkboxes toggle exactly their source marker;
  stale requests cannot overwrite newer content.
- [x] Resource toggles persist after reload and preserve comment anchors.
- [x] Editor toggles update the dirty buffer, support undo and persist on Save.
- [x] Registered callouts render in Resources and editor previews without
  changing stored Markdown; unregistered and nested markers retain fallback rendering.
- [x] Delegate focused native checks for checklist/callout behavior, editor input,
  Markdown selection, scrolling, Review and the tutorial WebView where accessible;
  distinguish confirmed behavior from gaps and existing failures in the results.

## Verification commands

```sh
cargo fmt --all -- --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo check --locked --no-default-features
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo test --locked --no-default-features
```

## Risks / open questions

- Fast rendering reuse can reveal missing notifications; verify visible updates
  rather than assuming that a passing build proves native behavior.
- Checkbox ranges refer to each TextView's source, including our display-only
  metadata insertions and per-block resource views. Mapping and stale-source
  checks must be tested before exposing persistence.
- Kit block surfaces handle registered top-level alert blockquotes and do not
  replace resource comment wrappers. Long or nested surfaces are out of scope.
- TLS lockfile presence does not prove a package is active on every platform;
  refresh the locked version without claiming an exposure audit.

## Results

Kit, Base, Component and macOS WebView resolve to 0.9.1. The lockfile contains
Fast 0.1.14, Pre 0.3.12 and rustls-webpki 0.103.16. Fast remains the default.
README and the editor/resources guides document the new behavior.

Resources saves checkbox changes through the existing revision-checked mutation.
Editor preview changes use an atomic, undoable buffer edit and explicit Save.
Both paths reject stale source and map display-only metadata breaks and nested
Unicode task offsets back to the original Markdown. Draft/save guards protect
resource edits, and task-only updates preserve the exact logical scroll offset.

The five shared callouts preserve original source and resource comment wrappers.
Resources find omits a hidden registered marker, including a leading empty quoted
line, while escaped/entity/title/unknown/nested forms retain ordinary fallback.

### Automated verification

| Check | Fast | Pre |
| --- | --- | --- |
| Full `cargo test --locked --offline` (Pre adds `--no-default-features`) | 877 unit + 8 CLI passed; 7 existing ignored | 877 unit + 8 CLI passed; 7 existing ignored |
| Strict all-target Clippy, `-D warnings` | Passed | Passed |
| Compilation | Test, Clippy and native debug build passed | Test and Clippy builds passed |

After the full runs, the additional `body_click_routes_find_through_text_view_focus`
regression passed separately on both backends. It clicks ordinary body text,
opens find through the TextView child's focus, and confirms a query match in
both reader modes without programmatically focusing Preview.

`cargo fmt --all -- --check` and `git diff --check` passed. Full tests used
approved local loopback access for the existing provider TCP test; the sandboxed
run had previously failed only at that bind. Regression tests cover source and
revision conflicts, comment anchors, editor dirty/undo/redo/save behavior,
both-reader scroll restoration, metadata/Unicode offsets and callout find/source
semantics. Logs are in the ignored `target/gpui-091-verification/` directory.

### Native observations and limits

The computer-use agent confirmed distinct Note/Warning callout surfaces with
comment affordances, nested Unicode task toggling and persistence across
navigation, retention of a bottom task's scroll position, resource scrolling,
comment draft opening/canceling, and command palette opening/dismissal. An
existing tutorial rendered in the WebView with its palette composed above it.
The disposable checklist fixture was restored, then removed with a
revision-checked CLI deletion.

On the final Fast build, body click followed by Cmd-F showed no find UI; a drag
over ordinary paragraph text followed by Cmd-C left the clipboard unchanged.
A focused pointer-selection diagnostic reproduced the same failure on Fast,
Pre, and an isolated unmodified 0.9.0 checkout (HEAD `8431f55`, plus only the
diagnostic test): selectable text and TextView focus existed, but selection and
copied text remained empty. This establishes the selection failure predates the
upgrade; it remains a follow-up, not a claimed improvement. The diagnostic is
retained under the ignored verification directory rather than added as a failing
application test. Existing programmatic find tests passed in both full suites,
and the added body-click find regression passed separately on both backends.
The isolated 0.9.0 diagnostic also opened find through the TextView's focus;
the native find observation remains unconfirmed by automated reproduction.

Native editor typing/undo and Review interaction remain unverified. Opening a
project from Home left one verification window unresponsive; a separate explicit
checkout launch allowed the reader checks. Only verification processes were
closed, leaving the installed application untouched. No performance measurements
were run, so no measured speedup is claimed.

### Review records

Plan artifact: `art-h2mg3utr`. Independent source review: `art-hzxxdaea`.
The reviewer found no unresolved upgrade correctness/security findings after
fixes for task scroll restoration, resource callout wrappers and hidden-marker
find indexing. The review artifact records verification evidence and native limits.
