#!/usr/bin/env python3
"""HiveMind transcript sidecar daemon.

Polls ~/.claude/projects/ for JSONL transcript changes (by mtime) and ships
new turns to /v1/ingest. Intended for hook-less harnesses (Codex, etc.) and
as a durability comparison against the hook shipper.

No external dependencies: stdlib only (os, time, json, urllib).

Configuration via environment variables:
  HIVEMIND_API_URL            Base URL of the HiveMind server (default: http://localhost:8080)
  HIVEMIND_API_KEY            Bearer token for /v1/ingest (optional in dev mode)
  HIVEMIND_AGENT_TOOL         agent_tool field (default: claude)
  HIVEMIND_SIDECAR_INTERVAL   Poll interval in seconds (default: 0.25)
  HIVEMIND_PROJECTS_DIR       Override ~/.claude/projects/ watch root

Usage:
  python3 capture/sidecar.py

  Or as a background process:
  python3 capture/sidecar.py &
  echo $! > /tmp/hivemind-sidecar.pid
"""

import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from core import ship


def main() -> None:
    api_url = os.environ.get("HIVEMIND_API_URL", "http://localhost:8080")
    api_key = os.environ.get("HIVEMIND_API_KEY", "")
    agent_tool = os.environ.get("HIVEMIND_AGENT_TOOL", "claude")
    poll_interval = float(os.environ.get("HIVEMIND_SIDECAR_INTERVAL", "0.25"))
    projects_dir = os.environ.get(
        "HIVEMIND_PROJECTS_DIR",
        os.path.expanduser("~/.claude/projects"),
    )

    # mtime cache: jsonl_path → last observed mtime
    mtimes: dict[str, float] = {}

    print(f"hivemind-sidecar: watching {projects_dir}", file=sys.stderr)

    try:
        while True:
            try:
                _scan(projects_dir, mtimes, api_url, api_key, agent_tool)
            except Exception as exc:
                print(f"hivemind-sidecar: scan error: {exc}", file=sys.stderr)
            time.sleep(poll_interval)
    except KeyboardInterrupt:
        pass

    print("hivemind-sidecar: stopped", file=sys.stderr)


def _scan(
    projects_dir: str,
    mtimes: dict,
    api_url: str,
    api_key: str,
    agent_tool: str,
) -> None:
    try:
        project_names = os.listdir(projects_dir)
    except OSError:
        return

    for project_name in project_names:
        project_path = os.path.join(projects_dir, project_name)
        if not os.path.isdir(project_path):
            continue
        try:
            fnames = os.listdir(project_path)
        except OSError:
            continue
        for fname in fnames:
            if not fname.endswith(".jsonl"):
                continue
            fpath = os.path.join(project_path, fname)
            try:
                mtime = os.path.getmtime(fpath)
            except OSError:
                continue
            if mtimes.get(fpath) == mtime:
                continue
            mtimes[fpath] = mtime
            session_id = fname[: -len(".jsonl")]
            ship(session_id, fpath, api_url, api_key, agent_tool=agent_tool)


if __name__ == "__main__":
    main()
