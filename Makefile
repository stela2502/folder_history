# FolderHistory release helper. GitHub Actions builds the platform binaries.
.DEFAULT_GOAL := help
.PHONY: help test build bump-patch bump-minor bump-major release status

PYTHON ?= python3
CARGO ?= cargo

help:
	@printf '%s\n' 'make test        Run release-mode tests' 'make build       Build release-mode binaries' 'make bump-patch Increment patch version and update Cargo.lock' 'make bump-minor Increment minor version' 'make bump-major Increment major version' 'make release     Commit version, create tag, push commit and tag' 'make status      Diagnose release state and next action'

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
