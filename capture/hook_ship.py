#!/usr/bin/env python3
"""Claude Code hook shipper.

Invoked by Claude Code PostToolUse and Stop hooks. Reads from the transcript
JSONL file at the cursor position, ships new turns to /v1/ingest, and exits 0.

Never blocks or raises: hook failures must not affect the agent.

Claude Code hooks receive a JSON payload on stdin (the documented hook
contract) with, at minimum, session_id, transcript_path, and cwd. That
payload is the authoritative source for session identity and transcript
location. Environment variables below are a fallback only, used when stdin
carries no payload (e.g. a manual invocation) or is missing a field.

Configuration via environment variables:
  CLAUDE_CODE_SESSION_ID  Fallback session id when stdin has none
  CLAUDE_PROJECT_DIR      Fallback project directory for path reconstruction
  HIVEMIND_API_URL        Base URL of the HiveMind server (default: http://localhost:8080)
  HIVEMIND_API_KEY        Bearer token for /v1/ingest (optional in dev mode)
  HIVEMIND_AGENT_TOOL     Overrides agent_tool field (default: claude)

Example .claude/settings.json hook configuration:
  {
    "hooks": {
      "PostToolUse": [
        {
          "matcher": "",
          "hooks": [
            {
              "type": "command",
              "command": "python3 /path/to/capture/hook_ship.py"
            }
          ]
        }
      ],
      "Stop": [
        {
          "hooks": [
            {
              "type": "command",
              "command": "python3 /path/to/capture/hook_ship.py"
            }
          ]
        }
      ]
    }
  }
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from core import jsonl_path_for_session, ship


def _read_hook_payload() -> dict:
    """Read and parse the hook's stdin JSON payload.

    Returns {} for an absent, empty, or non-JSON payload (e.g. manual
    invocation with no piped stdin) so the caller falls back to env vars.
    """
    if sys.stdin.isatty():
        return {}
    try:
        raw = sys.stdin.read()
    except Exception:
        return {}
    if not raw.strip():
        return {}
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return {}
    return data if isinstance(data, dict) else {}


def main() -> None:
    payload = _read_hook_payload()

    session_id = str(payload.get("session_id") or "").strip()
    if not session_id:
        session_id = os.environ.get("CLAUDE_CODE_SESSION_ID", "").strip()
    if not session_id:
        return

    api_url = os.environ.get("HIVEMIND_API_URL", "http://localhost:8080")
    api_key = os.environ.get("HIVEMIND_API_KEY", "")
    agent_tool = os.environ.get("HIVEMIND_AGENT_TOOL", "claude")

    transcript_path = str(payload.get("transcript_path") or "").strip()
    if transcript_path:
        jsonl = os.path.expanduser(transcript_path)
    else:
        project_dir = (
            str(payload.get("cwd") or "").strip()
            or os.environ.get("CLAUDE_PROJECT_DIR", "").strip()
            or os.getcwd()
        )
        jsonl = jsonl_path_for_session(session_id, project_dir)

    ship(session_id, jsonl, api_url, api_key, agent_tool=agent_tool)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        pass  # Never block the hook
    sys.exit(0)
