# GPUI Kit 0.8.1 adoption

## Goal

Adopt GPUI Kit 0.8.1 to improve Devcroft's existing readers, editors, scrolling,
and keyboard interaction, then use its new styling and Diff hooks where they
solve a concrete product problem. Keep GPUI Fast as the default backend.

Implemented on 2026-10-26. Kit and WebView resolve to 0.8.1, Fast to 0.1.12,
and Pre to 0.3.10. Shared reader styling and Git Diff syntax colors are enabled.
Both backends pass builds, full serial tests, and strict Clippy. Delegated native
verification is recorded below. Performance has not been measured.
The [completed 0.8.0 upgrade](devcroft:artifact/art-srqsk38k) supplies the existing
backend/overlay acceptance criteria and records earlier verification limits.

## Non-goals

- Replace Review's custom stream or the text comparison tool with Kit Diff.
- Rewrite Git loading, patch serialization, comment anchors, or truncation policy.
- Change the default backend or adopt the JavaScript shell.
- Treat upstream performance fixes as measured Devcroft speedups.

## Files that own this behavior

| Files | Responsibility |
| --- | --- |
| `Cargo.toml`, `Cargo.lock`, `mise.toml`, `README.md` | Kit normal/dev dependencies, macOS WebView version, shared backend feature, Rust toolchain, documented versions |
| `src/preview.rs`, `src/preview/resource_blocks.rs` | Shared Markdown reader, theme styling, nested artifact blocks, find, selection, comment source offsets |
| `src/editor/native.rs`, `src/tools/view.rs`, `src/git/merge_editor.rs` | Kit editors, previews, tool input/output, merge editing |
| `src/diff/kit_viewer.rs`, `src/diff/patch.rs`, `src/git/diff.rs` | Kit Diff in Git Changes and its bounded patch bridge |
| `src/diff/syntax.rs`, `src/review/mod.rs`, `src/review/stream.rs`, `src/tools/diff.rs` | Existing syntect palette/cache and custom review/comparison renderers |
| `src/main.rs`, `src/workspace.rs`, `src/backend_benchmarks.rs`, `src/review/tests.rs`, `src/pane.rs`, `src/repository_graph/canvas.rs` | Keybindings and scroll event compatibility |
| `src/artifacts.rs`, `src/artifacts/comments.rs`, `src/review/feedback.rs`, `src/git/view/commit_ui.rs` | Textareas and dialogs used for artifacts, comments, and commits |
| `src/tutorial_view.rs`, `src/webview/mod.rs` | Embedded WebView and backend-specific overlay behavior |

## Approach

### 1. Upgrade together and collect automatic fixes — highest priority

- [x] Raise both Kit dependency declarations from 0.8.0 to 0.8.1; align the exact
  macOS `gpui-webview` pin to `=0.8.1`, subject to registry availability. Refresh
  the lockfile with a targeted update and update README's version statement.
- [x] Preserve the existing feature coupling between Kit and WebView. The
  previous lockfile had Fast 0.1.10 and Pre 0.3.9; the release's upstream baselines
  are Fast 0.1.12 and Pre 0.3.10. Inspect the resolved versions and duplicate
  backend types. The tagged WebView still uses `lb-wry` 0.53.3, so this does not
  call for a separate Wry upgrade. Aligning WebView is a coordinated maintenance
  choice, not a claim that its old caret dependency necessarily prevents reuse.
- [x] Migrate `PlatformInput::ScrollWheel` to `PlatformInput::Scroll` in
  `src/backend_benchmarks.rs`, and rename `ScrollWheelEvent` imports/literals
  to `ScrollEvent` there and in `src/review/tests.rs`. Migrate deprecated
  production scroll event/listener aliases in the terminal, repository graph,
  and Review to `ScrollEvent` / `on_scroll`. Audit scroll literals
  for the new `source` field; the literals found already use
  `..Default::default()`, so they should inherit it.
- [x] Fix four test closures in `src/workspace.rs` that pass `key: &str` to
  `Keystroke::parse`. Use `&'static str` for helpers called only with literals,
  or `Keystroke::parse_with_os(key, cx.platform_info().os)` for runtime strings.
  Existing production bindings use static strings and explicit `cmd`/`ctrl`.
  No current calls to the changed modifier helpers or `unparse` were found;
  confirm through compilation rather than rewriting unaffected bindings.
- [x] Raise the minimum Rust version and mise pin to 1.98.0. Both new macOS
  backends call `Option::map_or_default`, which is unstable on the previous
  Rust 1.97.1 toolchain and stable from 1.98. The initial build exposed this
  requirement; use stable Rust rather than patching dependencies or bootstrap.

The upgrade should bring better selection gestures and table selection to the
shared Markdown reader, more accurate tab layout and horizontal extents to Kit
editors, and focus/scroll fixes to existing controls. Verify these on our actual
hosts. Selection snapshot batching and dropped-view text cleanup are especially
relevant to Resources, which creates nested retained TextViews; any reduction in
render work or retained memory remains an expectation to measure.

Upstream deliberately deferred the Textarea associated-type change and new
MotionTokens fields. Our numerous Textarea users do not need a speculative
rewrite for those changes.

### 2. Improve shared Markdown styling — highest-value new API

- [x] Replace the explicit inline-code limitation in `src/preview.rs` with
  `TextViewStyle::with_inline_code_style`: use the bundled monospace font,
  restrained horizontal padding, rounded corners, and theme-aware colors.
  The old `with_inline_code(HighlightStyle)` fallback can coexist where useful.
- [x] Apply the shared style to outer readers and nested resource blocks so
  standalone preview, editor preview, and Resources remain consistent.
- [x] Use `with_blockquote_style` for clearer quote bars/backgrounds and
  `with_list_style` for marker contrast. The latter styles bullets, numbers,
  and task markers; it is not a general list-body or indentation API.

The new inline style supports typography, padding, borders, and corner radii,
directly addressing the current code comment that waits for these capabilities.
Paragraphs still own line height. Validate wrapping, selection/copy, and comment
offsets instead of altering Markdown source to achieve presentation.

### 3. Share syntax colors with Kit Diff — useful optional follow-up

- [x] Add a small adapter in `src/diff/syntax.rs` and attach it to the
  `DiffState` owned by `KitDiffViewer`, so Git Changes can use the same syntect
  palettes as Review and the text comparison tool.
- [x] Map Kit's language identifiers to shared syntax names, using explicit
  `DiffFile::with_language` overrides where needed. Keep unknown languages
  readable. Include Swift's bundled syntax and an alias-heavy language fixture.
- [x] Return UTF-8 byte ranges relative to the callback's entire hunk-side
  string, rather than reusing our existing line-relative spans unchanged.
  The callback runs in the background and requires `Send + Sync`; capture
  palette data, not UI context. Preserve separate parser state per hunk side.
- [x] Install a stable callback and replace it with `set_syntax_highlighter`
  when theme colors change. Do not reset files or reinstall it every render.
  Upstream preserves selection and scroll when changing the highlighter.

This improves consistency in Git Changes. Review and text comparison already
have custom syntect renderers, so the hook does not automatically change those
surfaces or remove their rendering code.

### 4. Keep other new APIs conditional

- **Truncated hunks:** retain strict `DiffFile::parse` in our current bridge.
  Devcroft drops whole hunks at its 4,000-line cap and serializes complete retained
  hunks with exact source bytes. `DiffParseOptions::new().truncated_hunks(true)`
  solves incomplete external patches, which this bridge does not currently
  produce. `DiffFile::is_truncated` cannot detect whole hunks omitted upstream
  of parsing; keep our `FileContent::Text::truncated` warning. A future external
  patch reader may use the opt-in parser, with explicit incomplete-data UI.
- **Editor scrollbars:** use `EditorState::set_scrollbar_renderer` and
  `gpui_kit::base::input::EditorScrollbarContext` only if a focused visual pass
  finds a consistency issue. Reuse the provided scrollbar handle/geometry.
  This hook belongs to editors and does not remove the Markdown TextView's
  existing 16px gutter-clipping workaround.
- **Images:** our reader already imports `gpui_kit::base::TextView`, whose 0.8.0
  API already has `image_source`. The new component-wrapper forwarding is not
  a new capability for this reader. Relative-image support would require a
  separate source-path/base-directory design; PreviewView currently receives
  content, not the document path.
- **Dialogs and components:** `Dialog::centered` is optional positioning polish,
  not required migration. Keep existing dialog sizing. DataTable, Toolbar,
  ColorPicker, localization, and shell additions have no strong immediate
  adoption target in the inspected code.

All three phases are implemented without replacing the existing Review or text
comparison renderers.

## Acceptance criteria

- [x] Both backend configurations build and test with a reviewed, reproducible
  lockfile; Kit/WebView use the same backend types.
- [x] Existing reader find/TOC, Devcroft reference links, selected-text comments,
  editor-preview draft preservation, keyboard navigation, and Git Diff tests pass.
- [x] A focused native UI pass checks dark reader styling, narrow-window
  wrapping, sparse table rows, nested code scrolling, and reader find.
- [ ] Complete the broader manual matrix: long-line horizontal scroll,
  double/triple-click drag and Shift-click, selection confined to a table cell,
  disabled focused-button Tab traversal, and dialogs on a small window.
- [x] Fast's tutorial WebView accepts input and composes beneath the command
  palette in the focused native smoke pass.
- [ ] Verify native Pre hide-while-overlay behavior and the broader Fast
  dialog/menu/sheet overlay matrix. Pre's launch tool did not return a window.
- [ ] If phase 2 ships, inline styles remain legible in both themes and narrow
  panes, and copied text/comment anchors contain the original document text.
- [x] Git Changes uses the shared palettes; theme changes retain the Diff entity,
  selection, mode, and wrap setting. Multiline syntax, Unicode ranges, and
  omitted-hunk warnings pass regression tests. Source review confirms the
  highlighter setter retains scroll state.

Delegate UI interaction and visual verification to `computer-use`, with these
specific fixtures and acceptance criteria. Do not assume an in-app browser.
Delegate review of implementation diffs to `reviewer`, which records its review
as a Devcroft artifact.

## Verification commands

Run from the repository using its managed toolchain after implementing phase 1:

```sh
mise exec -- cargo fmt --all -- --check
mise exec -- cargo check --locked --all-targets
mise exec -- cargo check --locked --all-targets --no-default-features
mise exec -- cargo test --locked -- --test-threads=1
mise exec -- cargo test --locked --no-default-features -- --test-threads=1
mise exec -- cargo clippy --locked --all-targets -- -D warnings
mise exec -- cargo clippy --locked --all-targets --no-default-features -- -D warnings
mise exec -- cargo tree --locked -d
```

Use serial tests because the preceding upgrade recorded an intermittent
filesystem lease-lock failure during parallel execution.

For a performance claim, compare the existing ignored `backend_benchmark_`
workloads before/after, keeping toolchain, fixtures, and backend constant:

```sh
mise exec -- cargo test --locked backend_benchmark_ -- --ignored --nocapture --test-threads=1
mise exec -- cargo test --locked --no-default-features backend_benchmark_ -- --ignored --nocapture --test-threads=1
```

These benchmarks cover test-platform CPU rendering, not native GPU presentation,
terminal throughput, memory retention, or WebView composition. Do not run them
solely to validate this assessment document.

## Risks / open questions

- Kit's own patch compatibility does not preserve re-exported upstream GPUI
  signatures. Tests and benchmark code must compile as well as the application.
- Registry resolution is confirmed. Rust 1.98 is required by the resolved
  macOS backends; builders using an older toolchain must update it.
- New inline padding/font metrics can change wrapping and selection geometry;
  selected-text source anchoring makes the Resources fixture important.
- Custom Diff highlighters store concrete colors. A missing theme-change update
  would leave stale syntax colors even though the rest of the UI changes theme.
- Upstream fixes do not extend selection across separate resource TextViews or
  change our custom Review renderer, terminal, or embedded HTML rendering.
- The preceding upgrade recorded a native-WebView-focused Cmd+K limitation and
  an unconditional sheet-hiding workaround. This release is not evidence that
  either can be removed; preserve those baseline distinctions during UI checks.

## Assessment review

An independent source review found the missing `ScrollWheelEvent` to
`ScrollEvent` migration; this plan now includes it. No material findings remain.
The [review record](devcroft:artifact/art-8232gmhj) also records that dependency
resolution, compilation, tests, and native UI verification awaited implementation.
All repository paths and required plan sections were checked for this document.

## Implementation verification

Source review is recorded in [the implementation review](devcroft:artifact/art-7vqntdvw).
The review's plain-text fast-path finding is addressed: unknown and plain-text
files use Kit's `text` language and avoid unnecessary syntect parsing.

New tests compare Kit's hunk-side byte ranges and colors with Review for Unicode,
CRLF, multiline comments, and a missing final newline. They also cover parser
reset, unknown languages, path aliases, Swift, and plain-text fallback. The
retained-viewer interaction test now changes both themes with an active selection.

Automated verification passed on macOS with the pinned stable Rust 1.98.0:

| Check | Fast (default) | Pre (`--no-default-features`) |
| --- | --- | --- |
| All-target compilation | Passed | Passed |
| Native application build | Passed | Passed |
| Full serial unit suite | 795 passed, 6 existing ignored | 795 passed, 6 existing ignored |
| CLI integration tests | 8 passed | 8 passed |
| All-target Clippy, warnings denied | Passed | Passed |

Formatting and `git diff --check` passed. The duplicate-dependency audit found
no duplicate GPUI packages. No backend performance benchmarks were run.

Delegated native verification used disposable Fast and Pre application bundles
under `/private/tmp`, leaving the installed application untouched. The dark
Markdown fixture has readable inline code (including Unicode), clear quote/list
styling, and correctly separated sparse table cells. At 900×700, the reading
column remains readable and wraps naturally. The code block scrolls independently;
after focusing reader prose, find locates `reader-destination-081` with a 1/1
match. Fast's HTML fixture accepts input, increments its button counter, and stays
visible beneath a dimmed command palette.

Native light-theme styling and clipboard readback were not verified. Theme
controls are available only during onboarding, so the smoke pass did not change
the user's device settings to reach light mode. Automated Light/Dark Diff and
selected-text/source-anchor tests passed. The broader gesture, table-cell
selection, disabled-button traversal, editor, and small-dialog matrix remains
manual; no performance or exhaustive native-interaction claim is made.
Pre native interaction and overlay behavior remain unverified: the path-based
launch tool hung for 476 seconds and was interrupted without returning an app
identity or window. This is a verification-tool limitation, not an observed
application failure. The existing Pre fallback and both-backend sheet workaround
remain unchanged.

## Sources

- [GPUI Kit 0.8.1 release and migration notes](https://github.com/longbridge/gpui-kit/releases/tag/v0.8.1)
- [Tagged workspace dependencies](https://github.com/longbridge/gpui-kit/blob/v0.8.1/Cargo.toml)
- [Tagged WebView manifest](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/webview/Cargo.toml)
- [Inline-code, list-marker, and blockquote style contracts](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/text/style.rs)
- [Diff highlighter callback and lifecycle](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/component/src/diff/state.rs)
- [Diff parser options](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/component/src/diff/parser.rs)
- [Diff truncation semantics](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/component/src/diff/document.rs)
- [Editor scrollbar geometry](https://github.com/longbridge/gpui-kit/blob/v0.8.1/crates/base/src/input/scrollbar_presentation.rs)
- [Rust Option API stabilization](https://doc.rust-lang.org/std/option/enum.Option.html#method.map_or_default)
