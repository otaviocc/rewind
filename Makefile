.PHONY: build run test clean fmt fmt-check lint audit check install uninstall

# `check` is what CI would run if CI were on. While the repository is private,
# GitHub Actions is disabled (see CLAUDE.md), so this is the gate: run it before
# every commit that closes an issue. It covers four of the five CI jobs; the
# MSRV check and the Linux/Windows test matrix are the two this machine cannot
# reproduce, and they come back with CI at M6.
check: fmt-check lint test audit

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

# Accepted advisories, with a reason for each, are in .cargo/audit.toml.
audit:
	cargo audit --deny warnings

install:
	cargo install --path . --locked --force

uninstall:
	cargo uninstall rewind
