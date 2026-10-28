# Install, update, and release Devcroft

## Install or update

### Homebrew on macOS

After the tap is enabled and the first release is published:

```sh
brew install --cask croftkind/tap/devcroft
brew upgrade --cask devcroft
```

The cask requires Apple Silicon, verifies the archive's checksum, installs `Devcroft.app` and links the same executable as `devcroft` on Homebrew's PATH. The desktop launcher invokes `devcroft app`; the CLI remains usable without a running desktop. No separate CLI build or Homebrew bottles are needed. Use Homebrew for subsequent upgrades and removal (`brew uninstall --cask devcroft`). App data is preserved.

When switching from a source or shell installation, quit Devcroft and move the existing `Devcroft.app` aside before the first Homebrew install. Remove the old `~/.local/bin/devcroft` symlink if it points to that bundle, so it does not shadow Homebrew's CLI. Keep the application data directory.

### Shell installer on Linux or macOS

Once a release has been published, run:

```sh
curl -fsSL https://github.com/croftkind/devcroft/releases/latest/download/install.sh | bash
```

The same command installs a fresh copy or replaces an existing install. It downloads the installer from the latest stable GitHub release, resolves that release's version, downloads the matching architecture archive and `SHA256SUMS`, checks its SHA-256 hash, and verifies the executable version before replacing the app. No Rust/Zig compiler, source checkout, GitHub login, or sudo is needed. You need Bash, curl, tar, and either `sha256sum` or `shasum`.

- Linux x86-64: installs `~/.local/bin/devcroft`, a launcher under `${XDG_DATA_HOME:-~/.local/share}/applications`, and the app icon. Uses Ubuntu 24.04's glibc baseline (2.39+); Alpine/musl and older glibc distributions are unsupported. A working Vulkan driver and the fontconfig, ALSA, X11/Wayland, and xkbcommon libraries are required. On Ubuntu 24.04, install `libasound2t64 libfontconfig1 libfreetype6 libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 libxcb1 libx11-xcb1 libvulkan1 mesa-vulkan-drivers` if missing.
- macOS 13+ on Apple Silicon: installs `/Applications/Devcroft.app`, falling back to `~/Applications` if `/Applications` is not writable. Links `~/.local/bin/devcroft` to the combined app/CLI executable inside the bundle. Rosetta terminals select the native Apple Silicon package. Intel Macs are unsupported. Bundles are ad-hoc signed, not notarized; manually downloaded apps may require **System Settings → Privacy & Security → Open Anyway**. The installer does not disable Gatekeeper.

Add `~/.local/bin` to your PATH for CLI use, then launch **Devcroft** from your desktop launcher or run `devcroft app`. Quit and reopen the app after updating. App settings, sessions, and repository records stay in their existing data directory.

To pin a version, including an intentional downgrade:

```sh
curl -fsSL https://github.com/croftkind/devcroft/releases/latest/download/install.sh | bash -s -- --version v1.2.3
```

Optional absolute-path overrides: `DEVCROFT_BIN_DIR` for the CLI, `DEVCROFT_APP_DIR` for the macOS bundle parent, and `XDG_DATA_HOME` for Linux launcher/icon files. Pass installer environment variables to `bash`, for example `curl ... | DEVCROFT_APP_DIR="$HOME/Applications" bash`. An install into a custom location must be updated with the same overrides.

Ubuntu 24.04 users can alternatively download the `.deb` and run `sudo apt install ./devcroft-vX.Y.Z-x86_64-unknown-linux-gnu.deb`. This installs the CLI and desktop launcher system-wide and declares runtime dependencies. Choose one installation method to avoid duplicate launchers or PATH entries.

Archives and checksums are also available directly on the [GitHub Releases page](https://github.com/croftkind/devcroft/releases). Checksums detect corrupt or mismatched downloads; they are distributed through the same GitHub release as the binaries.

## New-release notice

The desktop checks the public GitHub latest-release endpoint in a background task at startup and every six hours while running. It only offers a strictly newer `vX.Y.Z` stable release. Drafts, prereleases, malformed responses, timeouts and network/rate-limit errors are ignored. It needs `curl` on PATH and sends no workspace data or credentials.

The Home notice has **Release notes**, **Copy update command**, and **Dismiss** actions. Dismissal lasts for the current Home view/app session and only hides that version. For Homebrew-managed bundles, the copied command is `brew upgrade --cask croftkind/tap/devcroft`; other installs receive the shell installer command above. Detection checks installed cask links under `/opt/homebrew`, `/usr/local`, and `HOMEBREW_PREFIX` when provided, without running Homebrew. The shell installer also refuses to replace a bundle owned by Homebrew. Updating is explicitly run by the user in their terminal. System `.deb` installs show **Download update** instead, opening the release assets so users can install the newer package through apt. Set `DEVCROFT_DISABLE_UPDATE_CHECK=1` in the desktop process environment to disable checks.

## Publish a release

Push a branch containing the release workflow:

```sh
git switch -c releases/v1.2.3
git push -u origin releases/v1.2.3
```

The branch name is the release version. Only `releases/vX.Y.Z` is accepted, without leading zeros, prerelease suffixes, or extra path segments. A branch must be pushed to GitHub; local creation alone does not trigger Actions. Manual **Run workflow** is also supported when selecting a release branch.

The workflow:

1. Validates the branch and tests release tooling.
2. Builds and tests on Ubuntu 24.04 x86-64 and macOS 15 Apple Silicon using the toolchains pinned in `mise.toml` and dependencies in `Cargo.lock`. Installs cargo-packager 0.11.8 with its locked dependencies.
3. Stamps `Cargo.toml`, `Packager.toml`, and the root package in `Cargo.lock` in each CI checkout with the branch version. Thus `devcroft --version`, internal version metadata, archive names and the macOS app version agree (cargo-packager uses a timestamp for the internal bundle build number). This does not commit or push a version bump. Source builds use the version in `Cargo.toml`; to reproduce a release from its tag, first run `python3 scripts/release/prepare.py releases/v1.2.3 --stamp`.
4. Uses `Packager.toml` to build native macOS `.app` bundles and a Linux `.deb`, including icons, metadata, launchers and ad-hoc macOS signatures. The small packaging helper assembles `.tar.gz` files for the shell installer and Homebrew.
5. After every build passes, creates the annotated `v1.2.3` tag at the triggering commit. Existing tags must already point to that commit; they are never moved.
6. Uploads both archives, the Linux `.deb`, `install.sh`, and `SHA256SUMS` to a draft release, verifies the uploaded file set/sizes, then publishes it with generated notes. An older maintenance version does not replace a newer stable release as Latest.
7. When `HOMEBREW_TAP` is configured, downloads the **published** checksums and updates `Casks/devcroft.rb` in the tap. Reruns use published checksums, not newly rebuilt archives, and older maintenance releases cannot downgrade the tap.

GitHub Actions needs permission to write repository contents for the publish job. The workflow uses `GITHUB_TOKEN`; publishing the release needs no personal access token or Apple signing secrets. Updating a separate Homebrew tap uses the scoped credential described below. Repository rules must permit the workflow to create release tags. Protect release branches as appropriate: a pushed release branch is an instruction to publish.

A failed build leaves no public release. A failed upload can leave a tag and draft release; rerun the same commit to finish. Rerunning an already published tag is a no-op. Pushing a different commit to the same release branch after tagging fails; use a new version branch for changes. Releases are serialized so two publishes cannot race to replace Latest. Do not force-push or reuse released versions.

When resuming a draft created by older tooling, the publisher removes its retired Intel macOS archive. The uploaded assets must exactly match the supported packages, installer, and checksums before the draft is published.

## Enable the Homebrew tap

Create the public GitHub repository `croftkind/homebrew-tap` with an initial commit (a README is sufficient). In the **Devcroft application repository**, configure:

- Actions variable `HOMEBREW_TAP`: `croftkind/homebrew-tap`.
- Actions secret `HOMEBREW_TAP_TOKEN`: a fine-grained token restricted to that tap repository with **Contents: read and write**. A GitHub App token may be used instead with the corresponding workflow setup.

The first successful release creates `Casks/devcroft.rb`; no manual version/checksum edits are needed. Until the variable is configured, the tap job is skipped and normal releases continue. An enabled but failed tap update marks that job failed while leaving the already-published release intact; rerun the job after correcting its configuration. The release workflow does not create repositories or credentials.

The cask template lives at `packaging/homebrew/devcroft.rb.in`. To generate a cask manually from a published release:

```sh
gh release download v1.2.3 --repo croftkind/devcroft --pattern SHA256SUMS --dir /tmp/devcroft-release
python3 scripts/release/homebrew.py v1.2.3 /tmp/devcroft-release/SHA256SUMS /path/to/homebrew-tap/Casks/devcroft.rb
```

The cask intentionally does not delete user data or bypass Gatekeeper. Apple Developer ID signing/notarization remains a separate setup step.

## Local packaging and checks

```sh
cargo install cargo-packager --version 0.11.8 --locked
cargo build --release --locked
# Use the version in Cargo.toml and the native target triple:
scripts/release/package.sh 0.1.0 aarch64-apple-darwin
```

The checked-in `packaging/icon.png` is a 512px derivative of `assets/icons/logo.png`, a size supported by cargo-packager's native icon conversion. `packaging/devcroft-app` is a shell launcher, not a second compiled application.

Tooling checks:

```sh
python3 -m unittest discover -s scripts/release -p 'test_*.py'
bash -n scripts/install.sh scripts/release/package.sh packaging/devcroft-app
shellcheck scripts/install.sh scripts/release/package.sh packaging/devcroft-app
actionlint .github/workflows/release.yml
cargo fmt --all -- --check
cargo test --locked
```
