# Restore TypeScript highlighting in Review

## Goal

Render TypeScript and TSX syntax colors in Review, including `.ts`, `.mts`, `.cts`, and `.tsx` files.

## Non-goals

Change editor grammars, language servers, diff layout, or the highlighting engine.

## Files that own this behavior

- `src/diff/syntax.rs`: shared syntax registry, path detection, and highlighting.
- `src/diff/kit_viewer.rs`: propagates the detected language into the retained diff viewer.
- `src/tools/diff.rs`: maps the shared language picker to the existing editor grammars.
- `assets/syntaxes/`: vendored grammars, licenses, and provenance.

## Approach

- [x] Trace Review's syntax detection and confirm the default syntect registry omits TypeScript.
- [x] Vendor syntect-compatible TypeScript and TSX definitions with pinned provenance and licenses.
- [x] Register both definitions alongside Swift without changing language detection for other files.
- [x] Add regression coverage for extension detection, both diff sides, both themes, and Kit's source highlighter.
- [x] Run focused checks and obtain an independent reviewer artifact.
- [x] Visually verify TypeScript and TSX diffs in the rebuilt app using temporary fixtures, then remove the fixtures.

## Acceptance criteria

- `.ts`, `.mts`, `.cts`, declaration variants, and `.tsx` resolve to the appropriate syntax.
- TypeScript keywords/types and TSX markup receive styles on additions and deletions in both themes.
- Kit receives the correct language and its highlighter returns valid byte ranges.
- Existing syntax tests continue to pass; unknown languages remain plain text.

## Verification commands

- `cargo fmt --all -- --check`
- `cargo test diff::syntax::tests -- --nocapture`
- `cargo test diff::kit_viewer::tests -- --test-threads=1`
- `cargo test tools::diff::tests -- --test-threads=1`
- `cargo build`

All commands above passed: 12 syntax tests, 3 retained Kit viewer tests, and 11 diff tool tests. Before the fix, the new extension regression failed because `src/main.ts` resolved to `None`. The pre-existing `block v0.1.6` future Rust compatibility warning remains.

Independent review found no blocking issues: [review](devcroft:artifact/art-uv3muk8z). The [implementation plan](devcroft:artifact/art-ba9vgup9) is also published in Devcroft.

Delegated native UI verification of the rebuilt debug app confirmed visible TypeScript keyword, comment, type-name, and string colors in Review > Uncommitted, plus TSX tag, attribute, literal, and expression colors. Evidence: `/private/tmp/devcroft-review-typescript-ui.png`. The two temporary source-checkout fixtures were removed; the debug app was closed and the original packaged app restored to the foreground. The existing packaged app was not replaced and needs a rebuild/relaunch to use this fix.

## Risks / open questions

- Newer Sublime syntax formats can exceed syntect's parser support; use grammars already consumed by a syntect application and test actual style output.
- Preserve upstream licensing and pin downloaded source revisions.
- Native GUI launch required execution outside the shell sandbox; the direct sandboxed process could not access the macOS pasteboard. This did not require a source change.
