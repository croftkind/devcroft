# Common language servers and Review highlighting

## Goal

Add C/C++ (clangd), HTML, CSS, JSON (VS Code language servers), YAML (yaml-language-server), Bash (bash-language-server), and Swift (SourceKit-LSP). Verify syntax highlighting for these files in Review independently of LSP installation.

## Non-goals

- Java, interactive snippet editing, unrelated LSP features, or automatic installation.
- Changing checkout trust, executable precedence, or existing server preferences.

## Files that own this behavior

- `src/editor/languages.rs`: file identity, highlighting grammar, and server routing.
- `src/editor/lsp/catalog.rs`: server names, launch policy, settings, and roots.
- `src/editor/lsp/install.rs`: pinned managed packages and launch resolution.
- `src/editor/lsp/settings.rs`: installation and runtime state in Settings.
- `src/editor/native.rs`: installation offer for toolchain-provided Swift support.
- `src/editor/lsp/client.rs`, `snippet.rs`, `providers.rs`: completion capability and conversion of supported snippets to plain-text edits (required by the HTML server).
- `assets/language-servers/`: integrity-locked npm recipes.
- `src/diff/syntax.rs`: Review syntax resolution and style regression coverage.
- `docs/language-support.md`: supported languages and prerequisites.

## Approach

- [x] Add catalog entries, language mappings, and bounded root detection.
- [x] Add pinned managed recipes and Node runtime handling; use official SHA-256-verified clangd archives where supported.
- [x] Add Bash with an integrity-locked npm recipe and Swift via an existing Swift/Xcode toolchain; missing Swift offers setup instructions instead of a managed install.
- [x] Adapt completion snippets to plain text, since the HTML server requires snippet capability to offer completion.
- [x] Cover routing, roots, managed launch, and Review additions/deletions in both themes with deterministic tests.
- [x] Exercise the new real servers in disposable directories and verify Review visually in the rebuilt app.
- [x] Obtain an independent reviewer artifact and address findings.

## Acceptance criteria

- Each language appears in Settings and routes to the appropriate server after checkout trust.
- Installs remain explicit, device-local, pinned, and isolated from projects.
- Missing runtimes cannot produce an enabled switch; native clangd has no Node runtime field.
- Standard C/C++ extensions and HTML/CSS/JSON/YAML/Bash/Swift files use matching editor grammar and LSP identifiers.
- Review colors representative syntax on additions and deletions in light and dark themes without a running LSP.
- Existing focused tests and build pass.

## Verification commands

- `mise exec -- cargo fmt --all -- --check`
- `mise exec -- cargo test --locked editor::languages::tests`
- `mise exec -- cargo test --locked editor::lsp:: -- --test-threads=1`
- `mise exec -- cargo test --locked diff::syntax::tests`
- `mise exec -- cargo test --locked common_servers_live_smoke -- --ignored --nocapture --test-threads=1`
- `mise exec -- cargo build --locked`
- Delegated native Settings and Review visual verification with temporary fixtures.

All checks passed: 123 editor tests (3 opt-in tests ignored), 13 shared Review syntax tests, and the explicit live smoke test covering eight language fixtures. Formatting, Clippy with warnings denied, build, and diff whitespace checks passed. The existing `block v0.1.6` future-compatibility notice remains.

Live checks exercised managed C/C++, HTML, CSS, JSON, YAML, and Bash installations and the installed Swift toolchain. All passed hover and completion. C/C++ and Swift passed definitions; C/C++, CSS, JSON, YAML, and Swift passed pushed diagnostics. Bash diagnostics were skipped because only an unconfigured ShellCheck shim was available; linting remains an optional external dependency.

Independent review: [review artifact](devcroft:artifact/art-y9m48ssw). Native verification confirmed logical cards for all new servers, no Node runtime/managed installation for Swift, and visible syntax colors for all eight languages in Review additions. Deterministic tests cover deletions and both themes. Evidence includes `/private/tmp/devcroft-common-lsp-review-top.png`, `/private/tmp/devcroft-common-lsp-review-lower.png`, `/private/tmp/devcroft-bash-swift-review.png`, and `/private/tmp/devcroft-swift-advanced.png`. Temporary repository fixtures were removed, the debug app closed, and the installed app restored without installing servers or saving preferences.

## Risks / open questions

- clangd needs compilation flags/compile_commands.json for project-accurate analysis; bundled binaries are platform-specific.
- HTML/CSS/JSON/YAML capabilities differ from source-language servers; validation must test each server's supported features rather than require definitions everywhere.
- Existing Review grammars must be checked by actual styled spans, not just successful extension detection.
- YAML remote catalogs are opt-in to keep configuration responsive on slow/offline connections. The smoke fixture uses a local schema and a non-dot temporary directory because upstream YAML glob matching skips dot directories.
- Swift uses the installed toolchain rather than a managed download; full cross-module results depend on project builds and build-server configuration.
