#!/usr/bin/env python3
"""Bump FolderHistory's Cargo version and publish a matching git tag."""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "Cargo.toml"
LOCK = ROOT / "Cargo.lock"
PACKAGE = "folder-history"


def run(*args: str) -> None:
    subprocess.run(args, cwd=ROOT, check=True)


def output(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def current_version() -> str:
    manifest = MANIFEST.read_text()
    package = manifest.split("[package]", 1)[1].split("\n[", 1)[0]
    match = re.search(r'^version\s*=\s*"(\d+\.\d+\.\d+)"', package, re.M)
    if not match:
        raise RuntimeError("Cannot find simple semver in Cargo.toml [package]")
    return match.group(1)


def check_version_files_clean() -> None:
    # Unrelated work is allowed during a version bump. Never overwrite
    # existing edits to either version file, including staged changes.
    changes = output("git", "status", "--porcelain", "--", "Cargo.toml", "Cargo.lock")
    if changes:
        raise RuntimeError(
            "Version files already modified; inspect them before bumping:\n" + changes
        )


def bump(kind: str) -> None:
    check_version_files_clean()
    old = current_version()
    major, minor, patch = map(int, old.split("."))
    if kind == "major":
        major, minor, patch = major + 1, 0, 0
    elif kind == "minor":
        minor, patch = minor + 1, 0
    else:
        patch += 1
    new = f"{major}.{minor}.{patch}"
    tag = f"v{new}"
    if subprocess.run(["git", "rev-parse", "--verify", "refs/tags/" + tag], cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        raise RuntimeError(f"Tag {tag} already exists")
    manifest = MANIFEST.read_text()
    package = manifest.split("[package]", 1)[1].split("\n[", 1)[0]
    updated = re.sub(r'(?m)^(version\s*=\s*")' + re.escape(old) + r'(")', lambda m: m.group(1) + new + m.group(2), package, count=1)
    MANIFEST.write_text(manifest.replace(package, updated, 1))
    lock = LOCK.read_text()
    pattern = r'(?m)(^name = "' + re.escape(PACKAGE) + r'"\nversion = ")' + re.escape(old) + r'("$)'
    updated_lock, count = re.subn(pattern, lambda m: m.group(1) + new + m.group(2), lock, count=1)
    if count != 1:
        MANIFEST.write_text(manifest)
        raise RuntimeError("Cannot find current package version in Cargo.lock")
    LOCK.write_text(updated_lock)
    print(f"Bumped {old} -> {new}. Review changes, then run: make release")


def release() -> None:
    version = current_version()
    tag = f"v{version}"
    branch = output("git", "symbolic-ref", "--quiet", "--short", "HEAD")
    if not branch:
        raise RuntimeError("Detached HEAD; refusing release")
    # Only the two version files may be modified; all other changes must be committed.
    changes = output("git", "status", "--porcelain").splitlines()
    if any(line[3:] not in ("Cargo.toml", "Cargo.lock") for line in changes):
        raise RuntimeError("Unrelated uncommitted changes; commit/stash first")
    if output("git", "tag", "--list", tag):
        raise RuntimeError(f"Tag {tag} already exists")
    if not changes:
        raise RuntimeError("No version changes to release; run make bump-patch/minor/major")
    # Avoid publishing a version that does not match the lockfile.
    if f'name = "{PACKAGE}"\nversion = "{version}"' not in LOCK.read_text():
        raise RuntimeError("Cargo.lock version does not match Cargo.toml")
    run("cargo", "test", "-r", "--locked")
    run("git", "add", "Cargo.toml", "Cargo.lock")
    run("git", "commit", "-m", f"Release {tag}")
    run("git", "tag", tag)
    run("git", "push", "origin", branch)
    run("git", "push", "origin", tag)
    print(f"Pushed {tag}; GitHub Actions will build the release.")


def main() -> None:
    if len(sys.argv) != 2 or sys.argv[1] not in ("show", "patch", "minor", "major", "release"):
        raise SystemExit("Usage: bump_version.py {show|patch|minor|major|release}")
    action = sys.argv[1]
    if action == "show":
        print(current_version())
    elif action == "release":
        release()
    else:
        bump(action)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(f"ERROR: {error}")
