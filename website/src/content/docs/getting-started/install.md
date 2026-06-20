---
title: Self-host
description: Install HiveMind on Linux or macOS and run your own server or local MCP.
---

This guide is for **hands-on builders** who want to run their own HiveMind instance.
If you want the fastest path, skip the install and
[connect your agent via the managed remote MCP server](/guides/mcp-setup/) instead —
no local install required.

---

## Install the binary

The fastest path is the installer script, which downloads the prebuilt binary for your
platform, verifies its SHA-256 checksum, and places `hivemind` in `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/alexknips/hivemind/master/scripts/install.sh | sh
```

Set `HIVEMIND_VERSION=v0.3.0` to pin a specific release, or `HIVEMIND_INSTALL_DIR=/usr/local/bin`
to choose a different destination.

## Prebuilt binaries

Tagged releases publish prebuilt tarballs for Linux and macOS on x86_64 and ARM64:

| Platform | Asset |
|----------|-------|
| Linux x86_64 | `hivemind-linux-x86_64.tar.gz` |
| Linux ARM64 | `hivemind-linux-arm64.tar.gz` |
| macOS x86_64 | `hivemind-macos-x86_64.tar.gz` |
| macOS ARM64 | `hivemind-macos-arm64.tar.gz` |

## Build from source

```bash
cargo install --git https://github.com/alexknips/hivemind --locked hivemind
```

Optional features:

```bash
# Persistent Kuzu graph projection
cargo install --git https://github.com/alexknips/hivemind --locked --features graph-kuzu hivemind

# Terminal UI
cargo install --git https://github.com/alexknips/hivemind --locked --features tui hivemind
```

## Verify

```bash
hivemind --version
hivemind --help
```

## Run the local MCP server

Once installed, use the local stdio MCP server with any MCP-aware client:

```bash
hivemind --hivemind-dir ./hivemind/ mcp
```

See [MCP Setup](/guides/mcp-setup/) for client configuration.

## Run the HTTP API server

```bash
HIVEMIND_DIR=./hivemind hivemind serve --port 8080
```

The server exposes all write and read operations over HTTP at `http://localhost:8080/v1/`.
Set `HIVEMIND_API_KEY` to require bearer-token authentication.

## Next steps

- [MCP Setup](/guides/mcp-setup/) — local stdio MCP configuration for all clients
- [Quickstart](/getting-started/quickstart/) — capture your first decision in one command
