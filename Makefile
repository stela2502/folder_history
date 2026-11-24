# FolderHistory release helper. GitHub Actions builds the platform binaries.
.DEFAULT_GOAL := help
.PHONY: help test build bump-patch bump-minor bump-major release status batch

PYTHON ?= python3
CARGO ?= cargo

help:
	@printf '%s\n' 'make test        Run release-mode tests' 'make build       Build release-mode binaries' 'make bump-patch Increment patch version and update Cargo.lock' 'make bump-minor Increment minor version' 'make bump-major Increment major version' 'make release     Commit version, create tag, push commit and tag' 'make status      Diagnose release state and next action' 'make batch ROOT=/path [JOBS=1] [OPTIONS=...]  Scan immediate subfolders'

test:
	$(CARGO) test -r --locked

build:
	$(CARGO) build -r --locked --bins

bump-patch:
	$(PYTHON) scripts/bump_version.py patch

bump-minor:
	$(PYTHON) scripts/bump_version.py minor

bump-major:
	$(PYTHON) scripts/bump_version.py major

status:
	@$(PYTHON) scripts/bump_version.py status

# State-aware: accepts a pending bump or an already committed version.
# Tests before tagging and atomically pushes branch and tag.
release:
	@$(PYTHON) scripts/bump_version.py release

# Scan immediate research subfolders; existing indexes are skipped by default.
JOBS ?= 1
OPTIONS ?=

batch:
	@test -n "$(ROOT)" || { echo "Usage: make batch ROOT=/path/to/parent [JOBS=1] [OPTIONS=...]" >&2; exit 2; }
	$(CARGO) build -r --locked --bin folder-history-cli
	./target/release/folder-history-cli sub-folders-of "$(ROOT)" --jobs "$(JOBS)" $(OPTIONS)
