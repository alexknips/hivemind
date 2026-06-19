---
title: Install
description: Install HiveMind on Linux or macOS.
---

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

## Next step

→ [Quickstart](/getting-started/quickstart/) — capture your first decision in one command
