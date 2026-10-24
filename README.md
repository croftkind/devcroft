# Devcroft

> Your personal development workspace

Devcroft is a GPUI based desktop app for personal development workflow. It focuses on keeping development workflow same no matter which harness you use. Vim like keyboard navigation let you fly between projects with ease.

## Quickstart

Install or update the latest release with the same command (available after the first release is published):

```sh
curl -fsSL https://github.com/hossainemruz/devcroft/releases/latest/download/install.sh | bash
```

macOS users can alternatively install through our Homebrew tap once it is enabled:

```sh
brew install --cask hossainemruz/tap/devcroft
brew upgrade --cask devcroft
```

Packages support Linux x86-64 (Ubuntu 24.04 or compatible) and macOS 13+ (Intel and Apple Silicon). The installer verifies checksums, installs the desktop app and CLI, and preserves your app data. Add `~/.local/bin` to your PATH for the CLI. Restart Devcroft after updating. Home shows a notice when a newer stable release is available. See [installation, platform requirements, and publishing releases](docs/releases.md).

You need Git (`git` on the application PATH) and at least one agent harness (`opencode`, `claude`, `codex`, or `omp`) on your default shell's PATH. Optional: `gh` (authenticated) for PR tracking, Neovim if you prefer it as your editor.

On a fresh installation, setup starts with **Sync & backup**: keep data local,
connect a new private Git repository, or choose **Existing repository** to connect
another device. A fresh setup then asks for a space, agent harnesses and account
isolation, editor and appearance, and an optional first repository. Repository
details use a collapsible accordion for the key, owner, name, clone URL,
description, and tags; Git detection fills the defaults.

Connecting another device skips space creation and the first repository step.
Your saved spaces, repositories, and isolation profiles are restored when you
finish; link their local checkouts from Home. Agent harnesses, default agent,
editor, and appearance are configured for this device. Restore validates the
backup before adoption and keeps original setup files locally for recovery.

Automatic sync can run every 2, 5, 15, or 30 minutes. For a new backup, verify access
with **Settings → Sync → Sync now** after setup. Setup does not install or
authenticate agent CLIs. Choices stay in a local draft until you finish, and
closing the app resumes setup next time. **Use defaults** opens a review of the
default choices. Existing installations keep their settings.

### Build from source

Install [mise](https://mise.jdx.dev/) to manage the pinned Rust and Zig toolchains:

```sh
mise install
mise run build
mise run app
```

To test onboarding with a clean installation, run `mise run app:fresh`. Each run
uses a new temporary data directory, which is removed when the app exits. Your
regular app data stays in its existing directory.

Install as the **Devcroft** desktop app:

```sh
mise run install          # release build; use --debug for a faster debug install
```

On Linux this installs `~/.local/bin/devcroft` plus a `devcroft.desktop` launcher; on macOS it (re)creates `/Applications/Devcroft.app` (`~/Applications` fallback when `/Applications` is not writable). Your data in `~/.local/share/devcroft` (`device.json`, `portable/`, caches) is kept. If the app ever aborts without a message, the report is written to `logs/panic.log` under the data root and is never synced.

## Features

- **Agent** runs your default harness (`opencode` unless changed in Settings → Agent) in your default login shell, resuming the checkout's most recent session when history exists. Use **New session** to pick a harness explicitly.
- **Git** opens as a full workspace page from **Open Git** in the header's branch menu, `Cmd/Ctrl+J`, then `g`, or **Open Git** in the actions palette. Back returns to the previous view. Changes, History, branch management, remote operations, and conflict editing share this page; dialogs open above it. See [Git workflow](docs/git.md).
- **Editor** offers Neovim or the built-in editor (Settings → Editor; the default for new installations). The built-in editor covers file tabs, fuzzy finding, project search, syntax highlighting, and find/replace for small edits. See [editor setup and everyday use](docs/editor.md) and [language support](docs/language-support.md).
- **Terminal** tabs own independent PTY sessions backed by your login shell, with bundled Nerd Font symbols and mouse/link support. See [Terminal](docs/terminal.md).
- **Home** dashboard for recent projects, repository relationships, PRs, todos, and reading lists, partitioned by spaces. See [Home dashboard](docs/home.md).
- **Review** tab with syntax highlighting and persistent line/range comments, plus a headless CLI for agents. See [review workflow and CLI](docs/review-comments.md).
- **Resources** tab for repository plans, RFCs, notes, reviews, and visual tutorials with agent-accessible comments. See [Repository resources](docs/resources.md).
- **Relationship graph** to connect providers to consumers across the active space. See [relationship graph](docs/relationships.md).
- **Tools** (action palette → Tools): Format JSON, Diff Checker, Base64 encoder/decoder. See [Developer tools](docs/tools.md).
- Keyboard-first throughout: `Cmd+J` on macOS (`Ctrl+J` elsewhere) enters navigation mode; `Cmd+K` / `Cmd+P` open the action and project palettes. See the [keyboard reference](docs/keyboard-reference.md).

## CLI

The CLI works without a running desktop. Records use `DEVCROFT_DATA_DIR` or the normal per-OS root.

```sh
devcroft repository list --json
devcroft artifact create --repository KEY --title "Implementation plan" --kind plan --content-file plan.md --json
devcroft review list --open
```

## Agent skill

In **Settings → Agent → Devcroft skill**, install the bundled CLI skill for Claude Code and/or Codex/shared agents (OpenCode discovers these locations too). The same installer works headless:

```sh
devcroft skill install
devcroft skill status
```

Modified or unmanaged skill folders are preserved. The maintained bundle lives in `assets/skills/devcroft/` and is embedded in the binary, so installation needs neither network access nor a source checkout.

## Development

```sh
mise run fmt
mise run check
mise run test
```

Ordinary builds use GPUI Fast. The default feature switches Kit and the macOS
WebView wrapper together so they share GPUI types.

GPUI Kit and gpui-webview use 0.8.0. Composed tutorial pages stay visible
behind palettes, menus, dialogs, and navigation hints on GPUI Fast. GPUI Pre
and hosts where native composition fails keep the hide-while-overlay fallback.
Sheets hide the tutorial on both backends because Kit does not compose them above
native content yet.

```sh
cargo run --locked -- app
cargo test --locked
```

To use GPUI Pre, disable the application's default features:

```sh
cargo run --locked --no-default-features -- app
cargo test --locked --no-default-features
```

To compare CPU rendering work, run the ignored reader/review benchmarks with
each backend, using the same toolchain and an idle machine. Repeat each command
several times; fixture construction and warmup are excluded from the reported
median and p95 milliseconds per input/render cycle.

```sh
# GPUI Pre
cargo test --locked --no-default-features backend_benchmark_ -- --ignored --nocapture --test-threads=1
# GPUI Fast
cargo test --locked backend_benchmark_ -- --ignored --nocapture --test-threads=1
```

These use GPUI's test platform. They do not measure native GPU presentation,
display pacing, terminal output, or embedded WebView behavior. Native performance
and interaction need separate verification.

## Docs

- [Home dashboard](docs/home.md)
- [Editor setup and everyday use](docs/editor.md)
- [Language support](docs/language-support.md)
- [Terminal](docs/terminal.md)
- [Repository resources](docs/resources.md)
- [Relationship graph](docs/relationships.md)
- [Review comments](docs/review-comments.md)
- [Developer tools](docs/tools.md)
- [Keyboard reference](docs/keyboard-reference.md)

## Contributing

GitHub Issues are disabled — please use GitHub Discussions for bug reports, feature requests, questions, ideas, and support. PRs are welcome, including AI-assisted ones: they must meet the same quality bar as any other contribution (run `mise run check`, describe what changed and how you verified it), and the author is responsible for everything in the PR. For security reports, see [Security Policy](SECURITY.md) instead of opening a discussion.

## License

MIT — see [LICENSE](LICENSE). Third-party font and icon attributions live in [assets/](assets/) (notably assets/icons/ATTRIBUTION.md).
