#!/usr/bin/env bash
# Build native packages with cargo-packager and assemble the installer archives.
# The Rust binary must already be built with the requested release version.
set -euo pipefail
version="${1:?version required}"
target="${2:?target required}"
[[ "$version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || exit 1
case "$target" in
  x86_64-unknown-linux-gnu) format=deb ;;
  aarch64-apple-darwin|x86_64-apple-darwin) format=app ;;
  *) echo "Unsupported target: $target" >&2; exit 1 ;;
esac
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
test "$(target/release/devcroft --version)" = "devcroft $version"
test "$(cargo packager --version)" = 'cargo-packager 0.11.8'
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p dist target/package-input
install -m755 target/release/devcroft target/package-input/devcroft
install -m755 packaging/devcroft-app target/package-input/devcroft-app
cargo packager --config Packager.toml --formats "$format" --target "$target"
install -m644 LICENSE "$work/LICENSE"
if [[ "$format" == app ]]; then
  codesign --verify --deep --strict dist/packages/Devcroft.app
  cp -R dist/packages/Devcroft.app "$work/Devcroft.app"
  test "$("$work/Devcroft.app/Contents/MacOS/devcroft" --version)" = "devcroft $version"
  test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$work/Devcroft.app/Contents/Info.plist")" = "$version"
else
  # Keep a compiler-free, non-root installer alongside the system package.
  install -m755 target/release/devcroft "$work/devcroft"
  install -m644 assets/icons/logo.png "$work/devcroft.png"
  install -m644 "dist/packages/devcroft-app_${version}_amd64.deb" "dist/devcroft-v${version}-${target}.deb"
fi
COPYFILE_DISABLE=1 tar -czf "dist/devcroft-v${version}-${target}.tar.gz" -C "$work" .
