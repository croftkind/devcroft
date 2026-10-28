Install or update Devcroft:

```sh
curl -fsSL https://github.com/croftkind/devcroft/releases/latest/download/install.sh | bash
```

Packages: Linux x86-64 (Ubuntu 24.04 or compatible glibc 2.39+ systems), macOS 13+ on Apple Silicon. Linux requires a working Vulkan driver, fontconfig, ALSA, X11/Wayland and xkbcommon runtime libraries. Git and your chosen agent harness must be on PATH. The installer verifies SHA-256 checksums before replacing the application. Restart Devcroft after updating.

macOS packages are ad-hoc signed, not Apple notarized. If macOS blocks a manually downloaded app, use System Settings → Privacy & Security → Open Anyway after verifying its source. The installer does not change Gatekeeper settings.

Homebrew on macOS (once the tap has been enabled):

```sh
brew install --cask croftkind/tap/devcroft
brew upgrade --cask devcroft
```

Ubuntu 24.04 users can also install the `.deb` asset using `sudo apt install ./devcroft-vX.Y.Z-x86_64-unknown-linux-gnu.deb`. Use the same package manager for subsequent updates.
