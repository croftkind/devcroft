#!/usr/bin/env python3
"""Render a cask from published checksums; never downgrade an existing tap."""
from pathlib import Path
import re
import sys

from prepare import VERSION

ROOT = Path(__file__).resolve().parents[2]


def render(tag, checksums):
    if not re.fullmatch("v" + VERSION, tag):
        raise ValueError("Expected tag vX.Y.Z")
    name = f"devcroft-{tag}-aarch64-apple-darwin.tar.gz"
    matches = [line.split()[0] for line in checksums.splitlines()
               if len(line.split()) == 2 and line.split()[1] == name]
    if len(matches) != 1 or not re.fullmatch(r"[a-f0-9]{64}", matches[0]):
        raise ValueError(f"Missing, duplicate or invalid checksum for {name}")
    replacements = {"VERSION": tag[1:], "ARM_SHA256": matches[0]}
    result = (ROOT / "packaging/homebrew/devcroft.rb.in").read_text()
    for key, value in replacements.items():
        result = result.replace(f"@{key}@", value)
    return result


def update(tag, checksums, destination):
    content = render(tag, checksums)
    if destination.exists():
        old = re.search(r'^  version "(' + VERSION + r')"$', destination.read_text(), re.MULTILINE)
        if not old:
            raise ValueError("Cannot determine the existing cask version")
        current = tuple(map(int, old[1].split(".")))
        incoming = tuple(map(int, tag[1:].split(".")))
        if current > incoming:
            print(f"Keeping newer Homebrew version {old[1]}")
            return
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(content)


if __name__ == "__main__":
    update(sys.argv[1], Path(sys.argv[2]).read_text(), Path(sys.argv[3]))
