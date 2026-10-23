#!/usr/bin/env python3
"""Validate a release branch and stamp only the CI checkout's package version."""
import argparse
import os
from pathlib import Path
import re

VERSION = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"


def branch_version(branch):
    if not re.fullmatch(r"releases/v" + VERSION, branch):
        raise ValueError("Release branch must be releases/vX.Y.Z (no leading zeros or suffixes)")
    return branch.removeprefix("releases/v")


def stamp(root, version):
    for name, pattern in [
        ("Cargo.toml", r'(\[package\]\nname = "devcroft"\nversion = ")[^"]+(")'),
        ("Cargo.lock", r'(\[\[package\]\]\nname = "devcroft"\nversion = ")[^"]+(")'),
        ("Packager.toml", r'(^version = ")[^"]+(")'),
    ]:
        # Match the root package, never a dependency with the same old version.
        path = root / name
        content, count = re.subn(pattern, lambda m: m[1] + version + m[2], path.read_text(), flags=re.MULTILINE)
        if count != 1:
            raise ValueError(f"Expected exactly one devcroft package in {name}")
        path.write_text(content)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("branch")
    parser.add_argument("--stamp", action="store_true")
    args = parser.parse_args()
    version = branch_version(args.branch)
    if args.stamp:
        stamp(Path(__file__).resolve().parents[2], version)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a") as stream:
            stream.write(f"version={version}\ntag=v{version}\n")
    print(version)


if __name__ == "__main__":
    main()
