#!/usr/bin/env python3
"""Create disposable, local-only checkouts for native Git visual verification."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(tempfile.mkdtemp(prefix="devcroft-git-ui-"))
env = os.environ.copy()
for key in list(env):
    if key.startswith("GIT_"):
        del env[key]
env.update(GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
           GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid",
           GIT_TERMINAL_PROMPT="0")


def git(checkout, *args, check=True):
    return subprocess.run(["git", "-C", str(checkout), *args], env=env,
                          capture_output=True, check=check)


def commit(checkout, message):
    git(checkout, "add", "-A")
    git(checkout, "-c", "core.hooksPath=/dev/null", "commit", "-m", message)


def repository(name):
    checkout = root / name
    checkout.mkdir()
    git(checkout, "init", "-b", "main")
    git(checkout, "config", "commit.gpgsign", "false")
    git(checkout, "config", "tag.gpgsign", "false")
    (checkout / "hello.rs").write_text('fn main() { println!("Hello"); }\n')
    (checkout / "merge.txt").write_text("base\n")
    commit(checkout, "Initial fixture commit")
    return checkout


clean = repository("clean")
dirty = repository("dirty")
(dirty / "hello.rs").write_text('fn main() { println!("Staged"); }\n')
git(dirty, "add", "hello.rs")
(dirty / "hello.rs").write_text('fn main() { println!("Unstaged"); }\n')
(dirty / "long path with spaces and 日本語.txt").write_text("untracked\n")
(dirty / "image.bin").write_bytes(b"\0binary fixture\n")
for i in range(270):
    (dirty / f"untracked-{i:03}.txt").write_text(f"file {i}\n")

history = repository("large-history")
for i in range(125):
    (history / "history.txt").write_text(f"revision {i}\n")
    commit(history, f"History fixture {i:03} with a longer descriptive subject")
git(history, "branch", "feature/a-long-branch-name-for-width-verification")
git(history, "tag", "fixture-v1")

conflict = repository("conflicted")
git(conflict, "checkout", "-b", "incoming")
(conflict / "merge.txt").write_text("incoming\n")
commit(conflict, "Incoming change")
git(conflict, "checkout", "main")
(conflict / "merge.txt").write_text("current\n")
commit(conflict, "Current change")
merged = git(conflict, "merge", "incoming", check=False)
assert merged.returncode == 1, "expected a merge conflict"

# A local bare origin produces real ahead/behind counts without network access.
diverged = repository("diverged")
origin = root / "origin.git"
git(diverged, "clone", "--bare", str(diverged), str(origin))
git(diverged, "remote", "add", "origin", str(origin))
git(diverged, "fetch", "origin")
git(diverged, "branch", "--set-upstream-to=origin/main")
peer = root / "peer"
git(diverged, "clone", str(origin), str(peer))
git(peer, "config", "commit.gpgsign", "false")
(peer / "remote.txt").write_text("remote\n")
commit(peer, "Remote change")
git(peer, "push", "origin", "main")
(diverged / "local.txt").write_text("local\n")
commit(diverged, "Local change")
git(diverged, "fetch", "origin")

print(root)
print("Fixtures: clean, dirty, diverged, large-history, conflicted")
print("Run: target/debug/devcroft app --checkout <fixture path>")
