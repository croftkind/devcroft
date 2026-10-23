"""Offline tests for release identity, immutable publishing, and real installer paths."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from prepare import branch_version, stamp
import publish
import homebrew

ROOT = Path(__file__).resolve().parents[2]


class ReleaseTests(unittest.TestCase):
    def test_strict_branch_contract(self):
        self.assertEqual(branch_version("releases/v10.20.30"), "10.20.30")
        for branch in ("main", "releases/v1.2", "releases/v01.2.3", "releases/v1.2.3-rc.1", "releases/v1.2.3/more", "releases/v1.2.3\n"):
            with self.assertRaises(ValueError):
                branch_version(branch)

    def test_stamp_changes_only_root_package(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("Cargo.toml", "Cargo.lock", "Packager.toml"):
                shutil.copyfile(ROOT / name, root / name)
            stamp(root, "12.34.56")
            import tomllib
            manifest = tomllib.loads((root / "Cargo.toml").read_text())
            before = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
            after = tomllib.loads((root / "Cargo.lock").read_text())["package"]
            self.assertEqual(manifest["package"]["version"], "12.34.56")
            self.assertEqual(tomllib.loads((root / "Packager.toml").read_text())["version"], "12.34.56")
            for old, new in zip(before, after, strict=True):
                if old["name"] == "devcroft":
                    self.assertEqual(new["version"], "12.34.56")
                else:
                    self.assertEqual(old, new)

    def test_publish_guards(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "dist").mkdir()
            with patch.object(publish, "run") as run:
                with self.assertRaisesRegex(ValueError, "Missing archive"):
                    publish.publish("v1.2.3", "abc", "owner/repo", root)
                run.assert_not_called()
            for target in publish.TARGETS:
                (root / "dist" / f"devcroft-v1.2.3-{target}.tar.gz").write_bytes(b"archive")
            (root / "dist/devcroft-v1.2.3-x86_64-unknown-linux-gnu.deb").write_bytes(b"deb")
            with patch.object(publish, "run", side_effect=["remote tag", "", "wrong commit"]):
                with self.assertRaisesRegex(ValueError, "another commit"):
                    publish.publish("v1.2.3", "abc", "owner/repo", root)
            with patch.object(publish, "run", side_effect=["remote tag", "", "abc"]) as run, patch.object(publish, "api", return_value={"draft": False}):
                publish.publish("v1.2.3", "abc", "owner/repo", root)
                self.assertEqual(run.call_count, 3)  # only read-only Git operations

    def test_publish_waits_for_complete_draft_and_preserves_newer_latest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "dist").mkdir()
            (root / "scripts").mkdir()
            (root / "scripts/install.sh").write_text("installer")
            for target in publish.TARGETS:
                (root / "dist" / f"devcroft-v1.2.3-{target}.tar.gz").write_bytes(b"archive")
            (root / "dist/devcroft-v1.2.3-x86_64-unknown-linux-gnu.deb").write_bytes(b"deb")

            def api(repo, endpoint):
                if endpoint == "releases/latest":
                    return {"tag_name": "v2.0.0"}
                return {"draft": True, "assets": [
                    {"name": p.name, "size": p.stat().st_size}
                    for p in (root / "dist").iterdir()
                ]}

            with patch.object(publish, "run", return_value="") as run, patch.object(publish, "api", side_effect=api):
                publish.publish("v1.2.3", "abc", "owner/repo", root)
                self.assertIn("--latest=false", run.call_args.args)
                self.assertIn("--draft=false", run.call_args.args)
            with patch.object(publish, "run", return_value="") as run, patch.object(publish, "api", return_value={"draft": True, "assets": []}):
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    publish.publish("v1.2.3", "abc", "owner/repo", root)
                self.assertFalse(any("--draft=false" in call.args for call in run.call_args_list))


class HomebrewTests(unittest.TestCase):
    def checksums(self, tag="v1.2.3"):
        return (f"{'a' * 64}  devcroft-{tag}-aarch64-apple-darwin.tar.gz\n"
                f"{'b' * 64}  devcroft-{tag}-x86_64-apple-darwin.tar.gz\n")

    def test_cask_uses_exact_architecture_checksums_and_cli_path(self):
        cask = homebrew.render("v1.2.3", self.checksums())
        self.assertIn('version "1.2.3"', cask)
        self.assertIn(f'arm:   "{"a" * 64}"', cask)
        self.assertIn(f'intel: "{"b" * 64}"', cask)
        self.assertIn('Contents/MacOS/devcroft", target: "devcroft"', cask)
        self.assertNotIn("@VERSION@", cask)
        with self.assertRaises(ValueError):
            homebrew.render("v1.2.3", self.checksums() + self.checksums())
        with self.assertRaises(ValueError):
            homebrew.render("v1.2.4", self.checksums())
        with self.assertRaises(ValueError):
            homebrew.render('v1.2.3"', self.checksums())

    def test_tap_rerun_and_maintenance_release_never_downgrade(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "Casks/devcroft.rb"
            homebrew.update("v1.10.0", self.checksums("v1.10.0"), destination)
            original = destination.read_bytes()
            homebrew.update("v1.2.3", self.checksums(), destination)
            self.assertEqual(destination.read_bytes(), original)
            homebrew.update("v1.10.0", self.checksums("v1.10.0"), destination)
            self.assertEqual(destination.read_bytes(), original)
            homebrew.update("v2.0.0", self.checksums("v2.0.0"), destination)
            self.assertIn('version "2.0.0"', destination.read_text())


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="devcroft install test ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.fake = self.root / "tools"
        self.fake.mkdir()
        self.downloads = self.root / "downloads"
        self.downloads.mkdir()
        self.bin_dir = self.root / "bin with spaces"
        self.data_dir = self.root / "data"
        self.app_dir = self.root / "Applications"
        self.env = dict(os.environ, PATH=f"{self.fake}:{os.environ['PATH']}",
                        DEVCROFT_BIN_DIR=str(self.bin_dir), XDG_DATA_HOME=str(self.data_dir),
                        DEVCROFT_APP_DIR=str(self.app_dir), FIXTURE_DIR=str(self.downloads),
                        FIXTURE_OS="Linux", FIXTURE_ARCH="x86_64")
        self.tool("uname", '#!/bin/sh\ncase "$1" in -s) echo "$FIXTURE_OS";; -m) echo "$FIXTURE_ARCH";; esac\n')
        self.tool("curl", '''#!/usr/bin/env python3
import os, pathlib, shutil, sys
args = sys.argv[1:]
if '--write-out' in args:
    print('https://github.com/hossainemruz/devcroft/releases/tag/v1.2.3', end='')
else:
    shutil.copyfile(pathlib.Path(os.environ['FIXTURE_DIR']) / args[-1].rsplit('/', 1)[-1], args[args.index('--output') + 1])
''')
        self.tool("update-desktop-database", "#!/bin/sh\nexit 0\n")
        self.tool("sysctl", "#!/bin/sh\necho 0\n")

    def tool(self, name, body):
        path = self.fake / name
        path.write_text(body)
        path.chmod(0o755)

    def archive(self, target="x86_64-unknown-linux-gnu", version="1.2.3", binary_version=None):
        name = f"devcroft-v{version}-{target}.tar.gz"
        binary = f'#!/bin/sh\necho "devcroft {binary_version or version}"\n'.encode()
        if "apple" in target:
            entries = {"Devcroft.app/Contents/MacOS/devcroft": binary,
                       "Devcroft.app/Contents/Info.plist": b"fixture"}
        else:
            entries = {"devcroft": binary, "devcroft.png": b"fixture icon"}
        with tarfile.open(self.downloads / name, "w:gz") as archive:
            for entry, content in entries.items():
                info = tarfile.TarInfo(entry)
                info.size = len(content)
                info.mode = 0o755 if entry.endswith(("devcroft", "devcroft-bin")) else 0o644
                archive.addfile(info, io.BytesIO(content))
        digest = hashlib.sha256((self.downloads / name).read_bytes()).hexdigest()
        (self.downloads / "SHA256SUMS").write_text(f"{digest}  {name}\n")
        return name

    def install(self, *args, success=True):
        result = subprocess.run(["bash", str(ROOT / "scripts/install.sh"), *args], env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def test_linux_install_update_and_preserve_data(self):
        self.archive()
        self.install()
        binary = self.bin_dir / "devcroft"
        self.assertEqual(subprocess.check_output([binary], text=True).strip(), "devcroft 1.2.3")
        desktop = (self.data_dir / "applications/devcroft.desktop").read_text()
        self.assertIn(f'Exec="{binary}" app', desktop)
        data = self.data_dir / "devcroft/portable/keep.txt"
        data.parent.mkdir(parents=True)
        data.write_text("keep")
        self.archive(version="1.2.4")
        self.install("--version", "v1.2.4")
        self.assertEqual(subprocess.check_output([binary], text=True).strip(), "devcroft 1.2.4")
        self.assertEqual(data.read_text(), "keep")

    def test_failed_integrity_and_version_leave_old_binary(self):
        name = self.archive()
        self.install()
        binary = self.bin_dir / "devcroft"
        original = binary.read_bytes()
        (self.downloads / name).write_bytes(b"corrupted")
        result = self.install(success=False)
        self.assertIn("Checksum mismatch", result.stderr)
        self.assertEqual(binary.read_bytes(), original)
        self.archive(binary_version="9.9.9")
        self.install(success=False)
        self.assertEqual(binary.read_bytes(), original)

    def test_macos_install_and_update_preserve_cli_target(self):
        self.env.update(FIXTURE_OS="Darwin", FIXTURE_ARCH="arm64")
        self.archive("aarch64-apple-darwin")
        self.install()
        binary = self.bin_dir / "devcroft"
        self.assertTrue(binary.is_symlink())
        self.archive("aarch64-apple-darwin", "1.2.4")
        self.install("--version", "v1.2.4")
        self.assertEqual(subprocess.check_output([binary], text=True).strip(), "devcroft 1.2.4")
        self.assertFalse(list(self.app_dir.glob(".devcroft-install.*")))

    def test_reject_unsupported_platform_and_bad_version(self):
        self.env["FIXTURE_ARCH"] = "aarch64"
        self.install(success=False)
        self.env["FIXTURE_ARCH"] = "x86_64"
        for version in ("v01.2.3", "v1.2.3/evil", "v1.2.3-rc1"):
            self.install("--version", version, success=False)
        self.assertFalse(self.bin_dir.exists())

    def test_macos_cli_failure_rolls_back_bundle(self):
        self.env.update(FIXTURE_OS="Darwin", FIXTURE_ARCH="arm64")
        self.archive("aarch64-apple-darwin")
        self.install()
        self.archive("aarch64-apple-darwin", "1.2.4")
        # Only fail the final CLI replacement; bundle moves still succeed.
        self.tool("mv", '''#!/bin/bash
if [[ "${!#}" == "$DEVCROFT_BIN_DIR/devcroft" ]]; then exit 1; fi
exec /bin/mv "$@"
''')
        self.install("--version", "v1.2.4", success=False)
        binary = self.bin_dir / "devcroft"
        self.assertEqual(subprocess.check_output([binary], text=True).strip(), "devcroft 1.2.3")
        self.assertFalse(list(self.app_dir.glob(".devcroft-install.*")))

    def test_macos_installer_refuses_to_replace_homebrew_managed_app(self):
        self.env.update(FIXTURE_OS="Darwin", FIXTURE_ARCH="arm64")
        self.archive("aarch64-apple-darwin")
        self.install()
        prefix = self.root / "homebrew"
        cask = prefix / "Caskroom/devcroft/1.2.3/Devcroft.app"
        cask.parent.mkdir(parents=True)
        cask.symlink_to(self.app_dir / "Devcroft.app", target_is_directory=True)
        self.env["HOMEBREW_PREFIX"] = str(prefix)
        self.archive("aarch64-apple-darwin", "1.2.4")
        result = self.install("--version", "v1.2.4", success=False)
        self.assertIn("brew upgrade --cask", result.stderr)
        self.assertEqual(subprocess.check_output([self.bin_dir / "devcroft"], text=True).strip(), "devcroft 1.2.3")


if __name__ == "__main__":
    unittest.main()
