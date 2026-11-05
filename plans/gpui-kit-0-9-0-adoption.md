# GPUI Kit 0.9.0 adoption

## Goal

Use GPUI Kit 0.9.0 to improve Devcroft's editors, Markdown readers and Review,
then adopt its new hooks where they improve existing workflows. Keep GPUI Fast
as the default backend.

Assessment date: 2026-11-04. Upgrade and feature adoption: 2026-11-05.
Status: implementation complete. Automated checks pass; native coverage limits
are recorded below.
Build and runtime results are recorded below.
The [release](https://github.com/longbridge/gpui-kit/releases/tag/v0.9.0), published
November 2, follows our completed [0.8.1 adoption](gpui-kit-0-8-1-adoption.md).
Current code takes precedence over that older plan: Review now uses Kit Diff
through `src/review/kit.rs`.

## Non-goals

- Replace the Review comment store, logical anchors, patch bridge or syntect highlighter.
- Rewrite the text comparison tool or change the default GPUI backend.
- Add hunk staging/reverting, runtime icon packs, speech or touch-specific features.
- Claim performance improvements without Devcroft measurements.

## Files that own this behavior

| Files | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `README.md`, `mise.toml` | Kit normal/dev versions, exact macOS WebView pin, backend feature coupling, documented versions and Rust toolchain |
| `src/main.rs`, `src/workspace.rs` | Theme initialization and switching; existing `Theme::update` customization |
| `src/review/kit.rs`, `src/review/feedback.rs`, `src/review/comments.rs`, `src/review/viewport.rs` | Kit Diff presentation, comment controls, durable anchors and selection restoration |
| `src/diff/kit_viewer.rs`, `src/diff/patch.rs`, `src/git/diff.rs` | Git Changes Diff and bounded source/patch mapping |
| `src/editor/native.rs`, `src/editor/native/git_markers.rs`, `src/editor/lsp/client.rs`, `src/editor/lsp/providers.rs`, `src/editor/lsp/transport.rs` | Editor event subscriptions, document synchronization, diagnostics and Unicode position conversion |
| `src/editor/project.rs`, `src/git/repository.rs` | Existing checkout/Git context to inspect when sourcing editor change baselines |
| `src/preview.rs`, `src/preview/resource_blocks.rs`, `src/git/merge_editor.rs`, `src/tools/view.rs` | Markdown readers and other Kit inputs/editors receiving upstream fixes |
| `src/caret_preferences.rs`, `src/settings.rs`, `src/data/device.rs` | Persisted caret preference |
| `src/tutorial_view.rs`, `src/webview/mod.rs`, `src/backend_benchmarks.rs` | WebView compatibility, overlay behavior and existing backend measurement support |

## Approach

### 1. Upgrade together and collect fixes — first change

- [x] Raise both Kit declarations from 0.8.1 to 0.9.0 and align the macOS
  `gpui-webview = "=0.8.1"` pin to `=0.9.0`, after checking registry availability.
  Refresh the lockfile with targeted updates; preserve the shared `gpui-fast`
  feature for Kit and WebView. Update README's version statement.
- [x] Inspect the resolved graph against the release baselines: Fast 0.1.13 and
  Pre 0.3.11, replacing our locked Fast 0.1.12 and Pre 0.3.10. Keep Rust 1.98.0
  unless compilation establishes a higher requirement. The upstream
  [workspace manifest](https://github.com/longbridge/gpui-kit/blob/v0.9.0/Cargo.toml)
  aligns Kit and WebView at 0.9.0.
- [x] Compile both backends and fix actual source incompatibilities. Public theme
  structs are now non-exhaustive: replace affected literals with constructors
  and field assignment. An initial search found no affected named literals in
  our sources/tests, and our focus theme already uses `Theme::update`; compilation
  must confirm the full impact. See [theme migration](https://github.com/longbridge/gpui-kit/pull/3713).
- [x] Audit upstream compatibility only where relevant: `Window::text_system()`
  now returns `Rc`, its window text system is no longer Send/Sync, and GPUI
  animation tests follow the executor clock. Existing `src/fonts.rs` calls use
  the app text system. No explicit `WindowTextSystem` or `catch_unwind` uses were
  found. See [backend changes](https://github.com/longbridge/gpui-kit/pull/3766).
- [x] Run the existing automated suites and focused delegated native checks on
  the readers, editors, diffs and overlays receiving upstream fixes. Coverage and
  remaining native checks are recorded below; this is not an exhaustive check
  of every upstream bug fix.

### 2. Improve Review comment visibility — first feature change

- [x] Use [Diff decorations and gutter markers](https://github.com/longbridge/gpui-kit/pull/3703)
  in `src/review/kit.rs`: supply `DiffDecoration` for the active comment range
  and `DiffGutterMarker` plus `render_gutter_marker` for existing comment threads.
  Clicking a marker should reveal/focus its thread without changing code selection.
- [x] Derive decorations and markers from existing path/side/line anchors each
  render. Preserve inline `DiffAnnotation` thread bodies and durable storage.
  Aggregate threads sharing a line into one accessible count/button because
  Kit displays only one marker per line. Keep missing/truncated-anchor fallbacks.
- [x] Preserve unified/split modes, old/new-side anchors, wrapping, keyboard
  activation, copy behavior and selection restoration. Decorations must not make
  additions/deletions unreadable. Defer an always-visible add-comment gutter
  until its discoverability and interaction are designed.

### 3. Adopt exact edit events, then editor change indicators

- [x] Subscribe to [InputEditsEvent](https://github.com/longbridge/gpui-kit/pull/3751)
  for editor synchronization. `subscribe_changes` now consumes immutable edit
  snapshots and routes them to the corresponding active or inactive document.
  Incremental servers receive precise ranges; full-sync servers retain full
  document notifications. Small edits have smaller notification payloads.
- [x] Replay immutable edit batches in order. Convert old byte ranges against
  the pre-edit document to LSP UTF-16 positions; extract inserted text from each
  event's corresponding post-edit Rope. Reuse the position conversion helpers.
  Retain full-sync support, document version/stale-response handling, and explicit
  full replacement for silent `set_value` paths. Cover inactive tabs, reopen,
  external reload and reconnect. Avoid duplicate notifications from the old
  Change subscription. Measure allocations, payload size and editing latency
  before claiming a speedup. The payload regression is measured; no allocation or
  latency improvement is claimed.
- [x] Add [gutter and overview-ruler decorations](https://github.com/longbridge/gpui-kit/pull/3704)
  to the built-in editor for added, modified and deleted lines. Compare the live
  buffer with the index as the baseline; show untracked text as added.
  Implement baseline loading and lifecycle explicitly: Kit paints/tracks ranges,
  but does not calculate Git changes. Refresh after edits and index/checkout
  changes, using background computation with stale-result rejection. Normalize
  the comparison consistently with repository line-ending and clean/smudge rules;
  raw LF index content versus a CRLF working buffer must not mark every line changed.
- [x] Use status colors with folded-line handling and empty boundary ranges for
  deletions. Keep a stable gutter width. Optionally add diagnostics in a separate
  ruler lane using the existing LSP diagnostic feed. Decision: retain the
  existing diagnostics UI without adding a ruler lane. Native checks cover the
  default dark theme; light-theme and folded/wrapped visual coverage remains a
  documented verification limit.

### 4. Optional polish after the main adoption

- [x] Consider a device-local caret preference using `Theme::cursor_blink`:
  steady, blinking or blinking with an idle timeout. Kit inputs share this policy;
  the Ghostty terminal needs its own integration. See [caret policy](https://github.com/longbridge/gpui-kit/pull/3713).
- [x] Consider [the icon resolver](https://github.com/longbridge/gpui-kit/pull/3633)
  only if we want runtime SVG icon themes. It can cover icons inside Kit controls
  and custom `svg()` icons through `resolve_icon_path`; it does not replace our
  PNG editor/agent branding pipeline or native-menu icons. Decision: retain the
  current icon pipeline; there is no runtime icon-theme workflow to serve.
- [x] Treat Kit Settings search/scrollspy and command-palette fixes as conditional:
  our settings dialog and palette logic are custom. Speech, Russian localization
  and touch selection are outside the current adoption scope. Decision: retain
  our custom settings and palette; collect their upstream fixes through Kit.

## Acceptance criteria

- [x] Both backend configurations compile and pass strict Clippy and existing tests.
- [x] The macOS tutorial WebView accepts input and preserves state across command
  palette dismissal with Fast composition and Pre's hide/restore fallback.
- [ ] Readers, merge inputs and editor hover/selection/resize behavior pass focused
  native checks. Upgrade alone preserves Review selection, comments and syntax colors.
- [ ] Comment markers reveal the correct thread with mouse and keyboard without
  replacing selection; shared-line counts and missing anchors behave correctly.
- [x] Incremental LSP notifications replay to the exact buffer after Unicode/emoji,
  multiline/CRLF, grouped edits, IME commit, undo/redo and programmatic reload;
  full-sync servers and stale-response protection still work.
- [ ] Git decorations reflect the chosen baseline and current unsaved buffer after
  save, stage/unstage, external modification and checkout changes; stale background
  results cannot decorate another document. Empty files and deletion at EOF work;
  LF/CRLF and supported clean/smudge filters cause no false whole-file changes.
- [x] Native UI inspection/interaction is delegated to the computer-use agent;
  a reviewer agent records its review as a Devcroft artifact for each non-trivial diff.

## Verification commands

Run after implementation, using the existing pinned toolchain:

```sh
cargo fmt --all -- --check
git diff --check
cargo build --locked
cargo build --locked --no-default-features
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
cargo test --locked --no-default-features -- --test-threads=1
cargo tree --locked -d
```

Inspect backend versions and Kit/WebView feature alignment in the graph rather
than treating every duplicate package as an error. Add targeted regressions for
the new edit replay, anchor projection and baseline invalidation behavior; do
not add tests that merely mirror builder calls. Native acceptance checks and any
performance measurements are separate from these commands.

## Upgrade verification — 2026-11-05

Kit and WebView resolve to 0.9.0, Fast to 0.1.13, and Pre to 0.3.11. Both builds
pass on the existing Rust 1.98.0 toolchain without application source changes.
Both backend test runs pass 851 unit tests (7 ignored) and all 8 CLI integration
tests. Formatting and diff whitespace checks pass. Strict all-target Clippy with
warnings denied passes on both backends. `cargo tree --locked -d` reports no
duplicate GPUI packages.

The [implementation review](devcroft:artifact/art-b3yr4p9j) reports no open
findings and records the same native coverage limits.

| Verification | Fast | Pre |
| --- | --- | --- |
| Build, Rust 1.98.0 | Passed | Passed |
| Unit tests | 851 passed, 7 ignored | 851 passed, 7 ignored |
| CLI integration tests | 8 passed | 8 passed |
| All-target Clippy, warnings denied | Passed | Passed |

Delegated native verification used separate temporary Fast/Pre app bundles,
leaving the installed application intact. Fast's HTML tutorial accepted text,
incremented its counter, and stayed visible behind the command palette; both
input and counter survived dismissal. The Markdown plan rendered and scrolled.
Fast Review displayed the Cargo.toml upgrade in unified and split modes with
syntax/change colors, scrolling, and the unified vertical scrollbar at the
viewport edge.

Pre's HTML tutorial hid while the palette was open and returned on Escape with
its input and counter intact. Its Markdown reader rendered headings, lists and
a table. Both temporary apps quit; a separate process check confirmed neither
remained running. The temporary tutorial artifact was deleted after verification.

Native reader find/selection, built-in editor selection/resize, Git Changes Diff,
Pre Diff, and the Settings overlay were not confirmed. The native Fast Diff
evidence covers Review, a separate surface from Git Changes. The broader native
acceptance item above remains unchecked. No performance measurements were run.

Screenshots and command logs are under `/private/tmp/devcroft-gpui-090/`, including
`fast-overlay.png`, `fast-review-unified.png`, `pre-overlay.png`,
`pre-restored.png`, and `pre-markdown.png`.

## Feature adoption — 2026-11-05

Review now renders an active comment range and accessible gutter buttons over
existing durable anchors. A unified context row combines old/new-side threads;
split mode keeps separate lanes. Repeated activation cycles shared-row threads,
reveals and focuses the thread, and preserves the current code selection.
Existing inline bodies and missing/outdated-anchor fallbacks remain in place.

Editor synchronization consumes immutable Kit edit snapshots in order and sends
UTF-16 incremental changes to servers requesting them. Full-sync servers receive
the final text. Silent replacement or a mismatched server baseline uses a full
replacement; version and stale-response protections remain. Regression coverage
includes queued edits after tab switching, undo/redo, Unicode/emoji, CRLF,
multiline batches and programmatic replacement. A single-character edit to a
100 KB document has under 200 bytes of change content. No latency or allocation
improvement is claimed.

Git gutters and the left overview-ruler lane show index-relative added,
modified and deleted ranges, including unsaved text. Reads and line comparison
run in the background; initial edits debounce for 150 ms. A single in-flight
job coalesces pending snapshots, rejects stale document/text results, and reuses
the filtered baseline for at most two seconds. Status colors, folded ranges,
empty deletion boundaries and a stable gutter strip use Kit collections.
Diagnostics remain in the existing diagnostic UI; the optional extra ruler lane
was deferred.

General settings offers Blink, Steady and Stop after idle (five seconds). The
choice applies live to Kit text fields and the built-in editor and persists in
device state across restarts. Unknown values retain Blink. Ghostty's separate
terminal cursor policy is unchanged.

Delegated native checks confirmed Fast Settings choices and reopen retention
(restored to Blink), built-in-editor overview marks and visible keyboard text
selection, and the Git Changes unified diff. Pre rendered Review in split mode
with shared-line comments and accepted accessibility activation of their marker.
Both temporary apps exited, confirmed by a separate process check. Temporary
comments c4/c5 were deleted through the CLI; no test text was inserted or saved.

Native marker keyboard activation, source-text copy/preservation, editor typing,
resizing/tab switching, cursor timing, Pre Settings overlay, and light-theme /
folded-text appearance were not confirmed. The earlier broad reader/merge/hover
acceptance also remains unverified; the artifact reader exposed no find control
when Cmd-F was tried. These are verification limits, not observed regressions.
The delegated agent reported screenshots with `new-fast-` / `new-pre-` prefixes.
Those temporary files are no longer present; the observations above are retained
from its verification report.

Final automated verification passed on the final working tree:

| Check | Fast | Pre |
| --- | --- | --- |
| Unit tests | 861 passed, 7 ignored | 861 passed, 7 ignored |
| CLI integration tests | 8 passed | 8 passed |
| All-target Clippy, warnings denied | Passed | Passed |
| Formatting / diff whitespace | Passed | Passed |

The final run includes the incremental IME commit and observable undo/redo
regression, full-sync compatibility, queued inactive-document synchronization,
unified old/new context-thread grouping, optional filter failures, and async
Git-marker document/text/index/close lifecycle checks. Both native app builds
also passed before delegated inspection. No performance claim is made beyond
the measured notification payload size.

Final checks were restarted after the prior tool processes and temporary logs
disappeared. Complete results/logs are retained in the ignored
`target/gpui-090-verification/` directory. The
[final feature review](devcroft:artifact/art-9qknc9s7) reports no open code findings
and preserves the native verification limits above.

## Risks / open questions

- Upgrade verification was performed on macOS with both backends. Remaining native
  checks are listed above; no cross-platform runtime or performance claim is made.
- Theme construction and upstream backend types are the main compatibility
  risks. Preserve Kit/WebView backend alignment and check the existing overlay
  acceptance criteria in the 0.8.1 plan.
- Editor markers use the index so staged changes are excluded and current
  unsaved/unstaged changes remain visible. Index/config/attribute changes are
  discovered by polling; the filtered baseline cache lives at most two seconds.
- Exact edit events expose post-edit snapshots; using those snapshots to convert
  old ranges would produce incorrect UTF-16 notifications. Silent replacements
  require explicit synchronization and baseline/decoration reset.
- Decorations track ranges through edits, but Git semantics still require diff
  recomputation. Debounce and reject stale results without delaying typing.
- Compare the filtered index to bytes the editor would save, preserving its
  line-ending convention and Git attributes. Failed/unsupported filters, binary
  data, unmerged index entries and buffers over 2 MiB hide indicators and show
  an unavailable status. Git reads are bounded; one worker prevents overlap.
- Selected lines take precedence over Diff decorations. Comment UI must remain
  understandable when the active range is also selected or has overlapping threads.
