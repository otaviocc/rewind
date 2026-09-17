.PHONY: build run test clean fmt fmt-check lint audit msrv check install uninstall

# Single-sourced from Cargo.toml, the way the CI job reads it, so the MSRV is
# stated in exactly one place.
MSRV := $(shell sed -n 's/^rust-version *= *"\(.*\)"/\1/p' Cargo.toml)

# `check` is what CI would run if CI were on. While the repository is private,
# GitHub Actions is disabled (see CLAUDE.md), so this is the gate: run it before
# every commit that closes an issue.
#
# `msrv` runs wherever rustup is available and skips where it is not, so this is
# the same command on both development machines and the Fedora box is the one
# that actually enforces the MSRV. Windows stays unverified until CI returns.
check: fmt-check lint test msrv audit

build:
	cargo build --release

run:
	cargo run -- --claude-dir tests/data/claude

test:
	cargo test

clean:
	cargo clean

fmt:
	cargo fmt

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --all-targets -- -D warnings

# Skips rather than fails without rustup: the macOS box is Homebrew rust, which
# cannot install a second toolchain. A missing toolchain *with* rustup present is
# a real failure, because that machine is expected to enforce this.
msrv:
	@if ! command -v rustup >/dev/null 2>&1; then \
		echo "msrv: skipped, no rustup on this machine (MSRV is enforced on the Linux box)"; \
	elif ! rustup toolchain list | grep -q '^$(MSRV)'; then \
		echo "msrv: toolchain $(MSRV) is not installed"; \
		echo "      run: rustup toolchain install $(MSRV)"; \
		exit 1; \
	else \
		cargo +$(MSRV) check --locked --all-targets; \
	fi

# Accepted advisories, with a reason for each, are in .cargo/audit.toml.
audit:
	cargo audit --deny warnings

install:
	cargo install --path . --locked --force

uninstall:
	cargo uninstall rewind
