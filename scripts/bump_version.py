#!/usr/bin/env python3
"""State-aware Cargo version bumping and GitHub tag publishing."""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / 'Cargo.toml'
LOCK = ROOT / 'Cargo.lock'
PACKAGE = 'folder-history'
VERSION_FILES = {'Cargo.toml', 'Cargo.lock'}


def command(*args, check=True):
    return subprocess.run(args, cwd=ROOT, text=True, capture_output=True, check=check)


def run(*args):
    print('+', ' '.join(args), flush=True)
    subprocess.run(args, cwd=ROOT, check=True)


def output(*args):
    return command(*args).stdout.strip()


def version():
    text = MANIFEST.read_text()
    match = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', text)
    if not match:
        raise RuntimeError('Missing [package] in Cargo.toml')
    found = re.search(r'^version\s*=\s*"(\d+\.\d+\.\d+)"', match.group(1), re.M)
    if not found:
        raise RuntimeError('Cannot find package semver')
    return found.group(1)


def changed_paths():
    raw = command('git', 'status', '--porcelain', '-z').stdout
    parts = raw.split('\0')
    paths = []
    i = 0
    while i < len(parts) and parts[i]:
        entry = parts[i]
        paths.append(entry[3:])
        if entry[:2][0] in 'RC' or entry[:2][1] in 'RC':
            i += 1  # rename/copy also reports original path
        i += 1
    return paths


def tag_exists(tag):
    return command('git', 'show-ref', '--verify', '--quiet', 'refs/tags/' + tag, check=False).returncode == 0


def remote_tag_exists(tag):
    result = command('git', 'ls-remote', '--exit-code', '--tags', 'origin', 'refs/tags/' + tag, check=False)
    if result.returncode not in (0, 2):
        raise RuntimeError('Cannot check remote tag on origin: ' + result.stderr.strip())
    return result.returncode == 0


def check_lock(v):
    if not re.search(r'(?m)^name = "' + re.escape(PACKAGE) + r'"\nversion = "' + re.escape(v) + r'"$', LOCK.read_text()):
        raise RuntimeError('Cargo.lock package version differs from Cargo.toml')


def bump(kind):
    dirty = set(changed_paths()) & VERSION_FILES
    if dirty:
        raise RuntimeError('Version files already modified: ' + ', '.join(sorted(dirty)))
    old = version()
    major, minor, patch = map(int, old.split('.'))
    if kind == 'major':
        major, minor, patch = major + 1, 0, 0
    elif kind == 'minor':
        minor, patch = minor + 1, 0
    else:
        patch += 1
    new = f'{major}.{minor}.{patch}'
    tag = 'v' + new
    if tag_exists(tag) or remote_tag_exists(tag):
        raise RuntimeError(f'{tag} already exists locally or on origin')
    manifest = MANIFEST.read_text()
    package = re.search(r'(?ms)^\[package\]\s*\n(.*?)(?=^\[|\Z)', manifest)
    updated_package = re.sub(r'(?m)^(version\s*=\s*")' + re.escape(old) + r'(")',
                             lambda m: m.group(1) + new + m.group(2), package.group(1), count=1)
    lock = LOCK.read_text()
    updated_lock, count = re.subn(r'(?m)(^name = "' + re.escape(PACKAGE) + r'"\nversion = ")' + re.escape(old) + r'("$)',
                                   lambda m: m.group(1) + new + m.group(2), lock, count=1)
    if count != 1:
        raise RuntimeError('Cannot locate package version in Cargo.lock; no files changed')
    MANIFEST.write_text(manifest[:package.start(1)] + updated_package + manifest[package.end(1):])
    LOCK.write_text(updated_lock)
    print(f'Bumped {old} -> {new}; run make release when ready.')


def state():
    v = version()
    tag = 'v' + v
    branch_result = command('git', 'symbolic-ref', '--quiet', '--short', 'HEAD', check=False)
    branch = branch_result.stdout.strip() if branch_result.returncode == 0 else None
    dirty = changed_paths()
    remote = remote_tag_exists(tag)
    local = tag_exists(tag)
    return v, tag, branch, dirty, local, remote


def status():
    v, tag, branch, dirty, local, remote = state()
    print(f'Version: {v}  Tag: {tag}  Branch: {branch or "DETACHED"}')
    print(f'Local tag: {"present" if local else "absent"}  Origin tag: {"present" if remote else "absent"}')
    print('Working tree:', 'clean' if not dirty else ', '.join(dirty))
    if remote:
        print('Next action: already published; bump the version for another release.')
    elif not branch:
        print('Next action: check out a branch before releasing.')
    elif set(dirty) - VERSION_FILES:
        print('Next action: commit/stash unrelated changes before release.')
    elif local:
        print('Next action: verify local tag and push it (make release).')
    else:
        print('Next action: test, commit version changes if any, tag and push (make release).')


def release():
    v, tag, branch, dirty, local, remote = state()
    check_lock(v)
    if not branch:
        raise RuntimeError('Detached HEAD; refusing release')
    unrelated = set(dirty) - VERSION_FILES
    if unrelated:
        raise RuntimeError('Unrelated changes must be committed/stashed: ' + ', '.join(sorted(unrelated)))
    if remote:
        if not local:
            raise RuntimeError(f'{tag} exists on origin but not locally; fetch tags and inspect')
        print(f'{tag} is already published. Nothing to do.')
        return
    if local and dirty:
        raise RuntimeError(f'{tag} is already tagged, but version files are modified; inspect first')
    if local:
        tagged_head = output('git', 'rev-list', '-n', '1', tag)
        head = output('git', 'rev-parse', 'HEAD')
        if tagged_head != head:
            raise RuntimeError(f'{tag} points to a different commit; refusing to move it')
    print(f'Releasing {tag} on {branch}', flush=True)
    run('cargo', 'test', '-r', '--locked')
    if dirty:
        run('git', 'add', '--', 'Cargo.toml', 'Cargo.lock')
        run('git', 'commit', '-m', f'Release {tag}')
    if not local:
        run('git', 'tag', tag)
    # Atomic push avoids publishing the tag without the corresponding branch.
    run('git', 'push', '--atomic', 'origin', branch, tag)
    print(f'Published {tag}; GitHub Actions can now build the binaries.')


def main():
    if len(sys.argv) != 2 or sys.argv[1] not in ('show', 'status', 'patch', 'minor', 'major', 'release'):
        raise SystemExit('Usage: bump_version.py {show|status|patch|minor|major|release}')
    action = sys.argv[1]
    if action == 'show':
        print(version())
    elif action == 'status':
        status()
    elif action == 'release':
        release()
    else:
        bump(action)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as exc:
        sys.exit(f'ERROR: {exc}')
