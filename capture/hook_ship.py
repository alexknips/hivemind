#!/usr/bin/env python3
"""Claude Code hook shipper.

Invoked by Claude Code PostToolUse and Stop hooks. Reads from the transcript
JSONL file at the cursor position, ships new turns to /v1/ingest, and exits 0.

Never blocks or raises: hook failures must not affect the agent.

Configuration via environment variables:
  CLAUDE_SESSION_ID       Claude Code session UUID (set by Claude Code hooks)
  CLAUDE_PROJECT_DIR      Project directory; falls back to cwd if unset
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

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from core import jsonl_path_for_session, ship


def main() -> None:
    session_id = os.environ.get("CLAUDE_SESSION_ID", "").strip()
    if not session_id:
        return

    project_dir = os.environ.get("CLAUDE_PROJECT_DIR", "").strip() or os.getcwd()
    api_url = os.environ.get("HIVEMIND_API_URL", "http://localhost:8080")
    api_key = os.environ.get("HIVEMIND_API_KEY", "")
    agent_tool = os.environ.get("HIVEMIND_AGENT_TOOL", "claude")

    jsonl = jsonl_path_for_session(session_id, project_dir)
    ship(session_id, jsonl, api_url, api_key, agent_tool=agent_tool)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        pass  # Never block the hook
    sys.exit(0)
