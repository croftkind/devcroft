#!/usr/bin/env bash
# Install/update the latest binary: curl -fsSL https://github.com/hossainemruz/devcroft/releases/latest/download/install.sh | bash
set -euo pipefail
repo="https://github.com/hossainemruz/devcroft"
version=""
fail() { echo "devcroft: $*" >&2; exit 1; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) [[ $# -ge 2 ]] || fail '--version needs vX.Y.Z'; version="$2"; shift 2 ;;
    *) fail "Unknown argument: $1 (supported: --version vX.Y.Z)" ;;
  esac
done
for tool in curl tar mktemp; do command -v "$tool" >/dev/null || fail "$tool is required"; done
case "$(uname -s)/$(uname -m)" in
  Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
  Darwin/arm64) target=aarch64-apple-darwin ;;
  Darwin/x86_64)
    # Prefer the native arm64 app even from a terminal running under Rosetta.
    if [[ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" == 1 ]]; then
      target=aarch64-apple-darwin
    else
      fail 'macOS packages require Apple Silicon'
    fi ;;
  *) fail 'Supported platforms: Linux x86-64, macOS Apple Silicon' ;;
esac
if command -v sha256sum >/dev/null; then
  checksum() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null; then
  checksum() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  fail 'sha256sum or shasum is required'
fi
fetch() {
  curl --disable --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 600 --retry 2 "$@"
}
if [[ -z "$version" ]]; then
  latest="$(fetch --output /dev/null --write-out '%{url_effective}' "$repo/releases/latest")"
  version="${latest##*/}"
fi
[[ "$version" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || fail "Invalid release version: $version"
archive="devcroft-${version}-${target}.tar.gz"
work="$(mktemp -d)"
staged=""
backup=""
dest=""
new_bundle=false
committed=false
cleanup() {
  if [[ "$committed" == false ]]; then
    if [[ -n "$backup" && -d "$backup" ]]; then
      # Restore on any later failure, including CLI link creation or interruption.
      if ! { rm -rf "$dest" && mv "$backup" "$dest"; }; then
        echo "Previous app preserved at $backup" >&2
        return
      fi
    elif [[ "$new_bundle" == true ]]; then
      rm -rf "$dest"
    fi
  fi
  rm -rf "$work"
  [[ -z "$staged" ]] || rm -rf "$staged"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "Downloading Devcroft $version ($target)..."
fetch --output "$work/$archive" "$repo/releases/download/$version/$archive"
fetch --output "$work/SHA256SUMS" "$repo/releases/download/$version/SHA256SUMS"
expected="$(awk -v name="$archive" '$2 == name { print $1 }' "$work/SHA256SUMS")"
[[ "$expected" =~ ^[a-f0-9]{64}$ ]] || fail 'Missing or invalid archive checksum'
[[ "$(checksum "$work/$archive")" == "$expected" ]] || fail 'Checksum mismatch; existing install was not changed'
mkdir "$work/unpack"
# Reject unsafe paths before extracting; archives contain only our fixed package layout.
tar -tzf "$work/$archive" > "$work/entries"
while IFS= read -r entry; do
  case "$entry" in /*|../*|*/../*|*/..) fail 'Unsafe archive path' ;; esac
done < "$work/entries"
tar -xzf "$work/$archive" -C "$work/unpack"
bin_dir="${DEVCROFT_BIN_DIR:-$HOME/.local/bin}"
[[ "$bin_dir" == /* ]] || fail 'DEVCROFT_BIN_DIR must be an absolute path'
if [[ "$target" == *-apple-darwin ]]; then
  source_app="$work/unpack/Devcroft.app"
  binary="$source_app/Contents/MacOS/devcroft"
else
  binary="$work/unpack/devcroft"
fi
[[ -x "$binary" && ! -L "$binary" ]] || fail 'Archive is missing its executable'
if ! actual_version="$("$binary" --version)"; then
  fail 'Binary cannot run on this system. See the release notes for OS and runtime requirements.'
fi
[[ "$actual_version" == "devcroft ${version#v}" ]] || fail 'Binary version does not match the release'
mkdir -p "$bin_dir"
if [[ "$target" == *-apple-darwin ]]; then
  app_dir="${DEVCROFT_APP_DIR:-/Applications}"
  if [[ -z "${DEVCROFT_APP_DIR:-}" && ! -w "$app_dir" ]]; then app_dir="$HOME/Applications"; fi
  [[ "$app_dir" == /* ]] || fail 'DEVCROFT_APP_DIR must be an absolute path'
  mkdir -p "$app_dir"
  dest="$app_dir/Devcroft.app"
  # Leave Homebrew's receipt and installed app consistent: updates belong to brew.
  for caskroom in /opt/homebrew/Caskroom /usr/local/Caskroom "${HOMEBREW_PREFIX:-/opt/homebrew}/Caskroom"; do
    for cask_app in "$caskroom"/devcroft/*/Devcroft.app; do
      if [[ -e "$dest" && "$cask_app" -ef "$dest" ]]; then
        fail 'This app is managed by Homebrew. Run: brew upgrade --cask hossainemruz/tap/devcroft'
      fi
    done
  done
  [[ ! -e "$dest" || ( -d "$dest" && ! -L "$dest" && -w "$dest" ) ]] || fail "Cannot replace $dest"
  staged="$(mktemp -d "$app_dir/.devcroft-install.XXXXXX")"
  cp -R "$source_app" "$staged/Devcroft.app"
  # Keep the previous bundle until the replacement is ready, and roll back on failure.
  backup="$staged/previous.app"
  if [[ -e "$dest" ]]; then mv "$dest" "$backup"; fi
  new_bundle=true
  mv "$staged/Devcroft.app" "$dest" || fail 'Could not replace app bundle'
  ln -s "$dest/Contents/MacOS/devcroft" "$staged/devcroft"
  # mv replaces the CLI entry itself, including a previous symlink.
  mv -f "$staged/devcroft" "$bin_dir/devcroft"
  committed=true
  echo "Installed $dest"
else
  data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
  [[ "$data_dir" == /* ]] || fail 'XDG_DATA_HOME must be an absolute path'
  [[ -f "$work/unpack/devcroft.png" ]] || fail 'Archive is missing its icon'
  staged="$(mktemp -d "$bin_dir/.devcroft-install.XXXXXX")"
  install -m755 "$binary" "$staged/devcroft"
  mv -f "$staged/devcroft" "$bin_dir/devcroft"
  mkdir -p "$data_dir/applications" "$data_dir/icons/hicolor/512x512/apps"
  install -m644 "$work/unpack/devcroft.png" "$data_dir/icons/hicolor/512x512/apps/devcroft.png"
  # Escape Desktop Entry string/Exec syntax (including paths containing spaces).
  desktop_bin="$bin_dir/devcroft"
  desktop_bin="${desktop_bin//\\/\\\\\\\\}"
  desktop_bin="${desktop_bin//\"/\\\\\"}"
  desktop_bin="${desktop_bin//\$/\\\\\$}"
  desktop_bin="${desktop_bin//\`/\\\\\`}"
  desktop_bin="${desktop_bin//%/%%}"
  rm -f "$data_dir/applications/Devcroft.desktop" "$data_dir/applications/com.devcroft.desktop.desktop"
  cat > "$data_dir/applications/devcroft.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=Devcroft
Comment=Devcroft workspace and agent CLI
Exec="$desktop_bin" app
Icon=devcroft
Terminal=false
Categories=Development;
StartupNotify=true
StartupWMClass=devcroft
EOF
  if command -v update-desktop-database >/dev/null; then update-desktop-database "$data_dir/applications" || true; fi
  echo "Installed $bin_dir/devcroft and desktop launcher"
fi
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "Add $bin_dir to your PATH to use the devcroft CLI." ;;
esac
echo "Devcroft $version installed. Quit and reopen Devcroft to use the new version."
