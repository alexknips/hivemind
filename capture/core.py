#!/usr/bin/env python3
"""Shared ingest-client core.

Reads new turns from a Claude Code JSONL transcript file, builds the
capture-classifier envelope, and ships it to POST /v1/ingest (fire-and-forget).

Both the hook shipper and the sidecar shipper call ship() from this module.
They behave identically — same payload schema, same cursor semantics, same
/v1/ingest call. Only the trigger differs.

Dependencies: stdlib only (json, os, urllib.request).
"""

import json
import os
import urllib.error
import urllib.request
from typing import Optional

# Per-turn text cap before we mark truncated=True.
_MAX_TURN_TEXT = 2000
# Maximum turns to include in a single batch.
_MAX_BATCH_TURNS = 4
# HTTP POST timeout in seconds.
_POST_TIMEOUT = 3


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def ship(
    session_id: str,
    jsonl_path: str,
    api_url: str,
    api_key: str,
    agent_tool: str = "claude",
) -> None:
    """Read new turns from jsonl_path since the last cursor and POST to /v1/ingest.

    Always returns None. Exceptions are caught and printed to stderr so the
    caller (hook or sidecar) is never blocked.
    """
    try:
        _ship_impl(session_id, jsonl_path, api_url, api_key, agent_tool)
    except Exception as exc:
        import sys
        print(f"hivemind-ingest: error for {session_id}: {exc}", file=sys.stderr)


def jsonl_path_for_session(session_id: str, project_dir: Optional[str] = None) -> str:
    """Return the JSONL transcript path for a session.

    Claude Code stores transcripts at:
      ~/.claude/projects/<project-hash>/<session_id>.jsonl

    where <project-hash> is the project directory path with '/' replaced by '-'.
    """
    if project_dir is None:
        project_dir = os.getcwd()
    project_hash = project_dir.replace("/", "-")
    return os.path.expanduser(f"~/.claude/projects/{project_hash}/{session_id}.jsonl")


def cursor_path_for_session(session_id: str) -> str:
    """Return the cursor state file path for a session."""
    return os.path.expanduser(f"~/.hivemind/cursors/{session_id}.offset")


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _ship_impl(
    session_id: str,
    jsonl_path: str,
    api_url: str,
    api_key: str,
    agent_tool: str,
) -> None:
    cursor_file = cursor_path_for_session(session_id)

    # Load cursor. On first run: initialise to EOF and return (skip history).
    try:
        with open(cursor_file, encoding="utf-8") as fh:
            cursor = int(fh.read().strip())
    except (OSError, ValueError):
        _init_cursor(cursor_file, jsonl_path)
        return

    # Check if there is new content.
    try:
        file_size = os.path.getsize(jsonl_path)
    except OSError:
        return
    if file_size <= cursor:
        return

    # Read new lines from cursor forward.
    turns = []
    new_cursor = cursor
    try:
        with open(jsonl_path, "rb") as fh:
            fh.seek(cursor)
            for raw_line in fh:
                new_cursor += len(raw_line)
                try:
                    obj = json.loads(raw_line)
                except json.JSONDecodeError:
                    continue
                turn = _extract_turn(obj)
                if turn is not None:
                    turns.append(turn)
    except OSError:
        return

    # Advance cursor regardless of POST outcome (fire-and-forget; no retry).
    _write_cursor(cursor_file, new_cursor)

    if not turns:
        return

    # Take the last N turns (most recent context).
    batch = turns[-_MAX_BATCH_TURNS:]
    batch_id = f"{session_id}:{cursor}-{new_cursor}"

    envelope = {
        "batch_id": batch_id,
        "agent_tool": agent_tool,
        "session_id": session_id,
        "turns": batch,
    }

    _post(api_url.rstrip("/"), api_key, envelope)


def _extract_turn(obj: dict) -> Optional[dict]:
    """Extract a turn dict from a JSONL record, or None if not a user/assistant turn."""
    turn_type = obj.get("type")
    if turn_type not in ("user", "assistant"):
        return None

    msg = obj.get("message")
    if not isinstance(msg, dict):
        return None

    role = msg.get("role", turn_type)
    content = msg.get("content", [])
    uuid = obj.get("uuid", "")

    text_parts = []
    truncated = False

    for item in content:
        if isinstance(item, str):
            if item.strip():
                text_parts.append(item.strip())
        elif isinstance(item, dict):
            item_type = item.get("type", "")
            if item_type == "text":
                text = item.get("text", "").strip()
                if text:
                    text_parts.append(text)
            elif item_type == "tool_use":
                name = item.get("name", "unknown")
                text_parts.append(f"[Tool: {name}]")
                truncated = True
            elif item_type == "tool_result":
                text_parts.append("[Tool result]")
                truncated = True
            # thinking and other types are skipped

    text = " ".join(text_parts).strip()
    if not text:
        return None

    if len(text) > _MAX_TURN_TEXT:
        text = text[:_MAX_TURN_TEXT] + "…"
        truncated = True

    return {
        "turn_id": uuid,
        "role": role,
        "text": text,
        "truncated": truncated,
    }


def _post(api_url: str, api_key: str, envelope: dict) -> None:
    """POST envelope to /v1/ingest. Raises on HTTP errors."""
    url = f"{api_url}/v1/ingest"
    body = json.dumps(envelope).encode()
    headers = {
        "Content-Type": "application/json",
        "X-HiveMind-Actor": "agent:claude:hook",
    }
    if api_key:
        headers["Authorization"] = f"Bearer {api_key}"

    req = urllib.request.Request(url, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=_POST_TIMEOUT) as resp:
            _ = resp.read()
    except urllib.error.HTTPError as exc:
        try:
            detail = exc.read(200).decode("utf-8", errors="replace").strip()
        except Exception:
            detail = ""
        raise RuntimeError(
            f"HTTP {exc.code} {exc.reason}" + (f": {detail}" if detail else "")
        ) from exc


def _init_cursor(cursor_file: str, jsonl_path: str) -> None:
    """Set cursor to current EOF so we only ship future turns."""
    try:
        offset = os.path.getsize(jsonl_path)
    except OSError:
        offset = 0
    _write_cursor(cursor_file, offset)


def _write_cursor(cursor_file: str, offset: int) -> None:
    os.makedirs(os.path.dirname(cursor_file), exist_ok=True)
    with open(cursor_file, "w", encoding="utf-8") as fh:
        fh.write(str(offset))
