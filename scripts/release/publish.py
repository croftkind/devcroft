#!/usr/bin/env python3
"""Publish complete native packages without moving tags or replacing public assets."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

from prepare import VERSION

TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin")


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def api(repo, endpoint):
    result = subprocess.run(
        ["gh", "api", f"repos/{repo}/{endpoint}"], text=True, capture_output=True
    )
    if result.returncode:
        if "(HTTP 404)" in result.stderr:
            return None
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def publish(tag, commit, repo, root):
    if not re.fullmatch("v" + VERSION, tag):
        raise ValueError("Expected tag vX.Y.Z")
    dist = root / "dist"
    archives = [dist / f"devcroft-{tag}-{target}.tar.gz" for target in TARGETS]
    archives.append(dist / f"devcroft-{tag}-x86_64-unknown-linux-gnu.deb")
    for archive in archives:
        if not archive.is_file() or archive.stat().st_size == 0:
            raise ValueError(f"Missing archive: {archive}")

    # Fetch the exact remote tag, if it exists; never force-update it.
    existing = run("git", "ls-remote", "--tags", "origin", f"refs/tags/{tag}")
    if existing:
        run("git", "fetch", "origin", f"refs/tags/{tag}")
        if run("git", "rev-parse", "FETCH_HEAD^{commit}") != commit:
            raise ValueError(f"{tag} already points to another commit; use a new release branch")
    release = api(repo, f"releases/tags/{tag}")
    if release and not release["draft"]:
        if not existing:
            raise ValueError("Published release is missing its tag; refusing to recreate it")
        print(f"{tag} is already published; leaving its assets unchanged")
        return

    shutil.copyfile(root / "scripts/install.sh", dist / "install.sh")
    assets = archives + [dist / "install.sh"]
    sums = dist / "SHA256SUMS"
    lines = []
    for path in assets:
        with path.open("rb") as stream:
            lines.append(f"{hashlib.file_digest(stream, 'sha256').hexdigest()}  {path.name}\n")
    sums.write_text("".join(lines))
    assets.append(sums)
    if not existing:
        run("git", "-c", "user.name=github-actions[bot]", "-c",
            "user.email=41898282+github-actions[bot]@users.noreply.github.com",
            "tag", "-a", tag, commit, "-m", f"Devcroft {tag}")
        run("git", "push", "origin", f"refs/tags/{tag}")
    if not release:
        run("gh", "release", "create", tag, "--repo", repo, "--verify-tag", "--draft",
            "--title", f"Devcroft {tag}", "--generate-notes", "--notes-file",
            str(root / "scripts/release/notes.md"))
    # Retrying a failed draft upload is safe. Published releases never reach here.
    run("gh", "release", "upload", tag, "--repo", repo, "--clobber", *map(str, assets))
    uploaded = api(repo, f"releases/tags/{tag}")
    expected = {path.name: path.stat().st_size for path in assets}
    actual = {asset["name"]: asset["size"] for asset in uploaded["assets"]}
    if any(actual.get(name) != size for name, size in expected.items()):
        raise ValueError("Draft assets are incomplete; release remains unpublished")
    # An older maintenance release must not replace a newer release as Latest.
    latest = api(repo, "releases/latest")
    latest_tag = latest["tag_name"] if latest else ""
    is_latest = not re.fullmatch("v" + VERSION, latest_tag) or (
        tuple(map(int, tag[1:].split("."))) > tuple(map(int, latest_tag[1:].split(".")))
    )
    run("gh", "release", "edit", tag, "--repo", repo, "--draft=false",
        f"--latest={str(is_latest).lower()}")
    print(f"Published {tag}")


if __name__ == "__main__":
    publish(*sys.argv[1:], root=Path(__file__).resolve().parents[2])
