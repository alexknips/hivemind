.PHONY: build test install fmt clippy clean

build:
	cargo build

test:
	cargo test

# Builds the `hivemind` CLI from this checkout and installs it to
# ~/.local/bin/hivemind — NOT cargo's default install root (~/.cargo/bin).
# On Gas Town rigs ~/.local/bin comes first on $PATH (it's also where
# scripts/install.sh puts published releases), so installing to
# ~/.cargo/bin instead is silently shadowed: `make install` "succeeds" but
# `hivemind` on PATH keeps resolving whatever stale binary is already in
# ~/.local/bin. This is exactly how the fleet ran a two-week-old build
# while master kept moving (hivemind-zdsh.7). Run this after every
# rebase/pull so agent tooling (MCP config, capture scripts, plugins) that
# resolves `hivemind` on PATH always finds the current version — confirm
# with `hivemind --version`, which now reports <semver>+<build sha>.
install:
	cargo install --path . --locked --bin hivemind --root "$(HOME)/.local"

fmt:
	cargo fmt

clippy:
	cargo clippy --all-targets -- -D warnings

clean:
	cargo clean
