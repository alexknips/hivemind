.PHONY: build test install fmt clippy clean

build:
	cargo build

test:
	cargo test

# Builds the `hivemind` CLI from this checkout and installs it into cargo's
# install root (~/.cargo/bin by default), which is on $PATH for a standard
# Rust toolchain install. Run this after every rebase/pull so agent tooling
# (MCP config, capture scripts, plugins) that resolves `hivemind` on PATH
# always finds the current version instead of a stale one.
install:
	cargo install --path . --locked --bin hivemind

fmt:
	cargo fmt

clippy:
	cargo clippy --all-targets -- -D warnings

clean:
	cargo clean
