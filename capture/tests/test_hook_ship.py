#!/usr/bin/env python3
"""Integration tests for capture/hook_ship.py against a real stub HTTP server.

Drives hook_ship.py as a subprocess the same way Claude Code does: JSON hook
payload on stdin, environment variables for the HiveMind server location.
Exercises the real hook contract (session_id/transcript_path from stdin, a
cwd containing dots) end to end, rather than monkeypatching internals.
"""

import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest

HOOK_SHIP = os.path.join(os.path.dirname(__file__), "..", "hook_ship.py")


class _StubIngestHandler(http.server.BaseHTTPRequestHandler):
    """Responds to POST /v1/ingest with whatever status the test queued."""

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        self.server.requests.append(json.loads(body))

        status = self.server.next_status_fn()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, *args):  # silence default request logging
        pass


class _StubIngestServer:
    """Background HTTP server for /v1/ingest, with a scriptable status queue."""

    def __init__(self):
        self.httpd = http.server.HTTPServer(("127.0.0.1", 0), _StubIngestHandler)
        self.httpd.requests = []
        self._statuses = [202]
        self.httpd.next_status_fn = self._next_status
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()

    def _next_status(self):
        if len(self._statuses) > 1:
            return self._statuses.pop(0)
        return self._statuses[0]

    def set_statuses(self, *statuses):
        self._statuses = list(statuses)

    @property
    def url(self):
        return f"http://127.0.0.1:{self.httpd.server_port}"

    @property
    def requests(self):
        return self.httpd.requests

    def stop(self):
        self.httpd.shutdown()
        self.thread.join(timeout=5)
        self.httpd.server_close()


class TestHookShipStdinContract(unittest.TestCase):
    """hook_ship.py must take session_id/transcript_path from stdin JSON,
    matching the documented Claude Code hook contract, not env vars."""

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        # A cwd with a dot, like every Gas City session
        # (/home/ubuntu/gc/.gc/agents/mayor) — the naive '/' -> '-' mangling
        # bug this bead fixes would miss transcripts under such a path.
        self.session_id = "session-with-dot-cwd"
        self.transcript_path = os.path.join(self.tmpdir, f"{self.session_id}.jsonl")
        self.cursor_home = os.path.join(self.tmpdir, "home")
        os.makedirs(self.cursor_home, exist_ok=True)

        self.server = _StubIngestServer()

    def tearDown(self):
        self.server.stop()

    def _write_transcript(self, records):
        with open(self.transcript_path, "w", encoding="utf-8") as fh:
            for record in records:
                fh.write(json.dumps(record) + "\n")

    def _run_hook(self, payload):
        env = dict(os.environ)
        env["HIVEMIND_API_URL"] = self.server.url
        env["HOME"] = self.cursor_home  # isolate ~/.hivemind/cursors
        result = subprocess.run(
            [sys.executable, HOOK_SHIP],
            input=json.dumps(payload).encode(),
            env=env,
            capture_output=True,
            timeout=15,
        )
        return result

    def test_ships_new_turns_via_stdin_payload(self):
        """A real-shaped hook stdin payload for a dotted cwd, against a stub
        /v1/ingest returning 202: new turns arrive and the cursor advances."""
        self._write_transcript(
            [
                {
                    "type": "user",
                    "uuid": "u1",
                    "message": {
                        "role": "user",
                        "content": [{"type": "text", "text": "hello"}],
                    },
                }
            ]
        )
        payload = {
            "session_id": self.session_id,
            "transcript_path": self.transcript_path,
            "cwd": "/home/ubuntu/gc/.gc/agents/mayor",
            "hook_event_name": "Stop",
        }

        self.server.set_statuses(202)

        # First invocation: cursor initialises to EOF, first-run policy
        # skips the existing content (matches sidecar/core semantics).
        result = self._run_hook(payload)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.server.requests, [])

        # Append a new turn and run again: it should ship.
        with open(self.transcript_path, "a", encoding="utf-8") as fh:
            fh.write(
                json.dumps(
                    {
                        "type": "assistant",
                        "uuid": "a1",
                        "message": {
                            "role": "assistant",
                            "content": [{"type": "text", "text": "hi back"}],
                        },
                    }
                )
                + "\n"
            )

        result = self._run_hook(payload)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(self.server.requests), 1)
        self.assertEqual(self.server.requests[0]["session_id"], self.session_id)
        self.assertEqual(self.server.requests[0]["turns"][0]["turn_id"], "a1")

        cursor_file = os.path.join(
            self.cursor_home, ".hivemind", "cursors", f"{self.session_id}.offset"
        )
        self.assertTrue(os.path.exists(cursor_file))
        with open(cursor_file, encoding="utf-8") as fh:
            offset = int(fh.read())
        self.assertEqual(offset, os.path.getsize(self.transcript_path))

    def test_failed_posts_do_not_advance_cursor_and_hook_never_blocks(self):
        """401, then 500, then an unreachable server: the cursor must not
        advance across any of them, the hook must still exit 0, and the
        next successful run must resend the same batch."""
        self._write_transcript(
            [
                {
                    "type": "user",
                    "uuid": "u0",
                    "message": {
                        "role": "user",
                        "content": [{"type": "text", "text": "seed"}],
                    },
                }
            ]
        )
        payload = {
            "session_id": self.session_id,
            "transcript_path": self.transcript_path,
            "cwd": "/home/ubuntu/gc/.gc/agents/mayor",
            "hook_event_name": "Stop",
        }

        # Init cursor to EOF (first run).
        result = self._run_hook(payload)
        self.assertEqual(result.returncode, 0)

        with open(self.transcript_path, "a", encoding="utf-8") as fh:
            fh.write(
                json.dumps(
                    {
                        "type": "assistant",
                        "uuid": "a1",
                        "message": {
                            "role": "assistant",
                            "content": [{"type": "text", "text": "reply"}],
                        },
                    }
                )
                + "\n"
            )

        cursor_file = os.path.join(
            self.cursor_home, ".hivemind", "cursors", f"{self.session_id}.offset"
        )
        with open(cursor_file, encoding="utf-8") as fh:
            offset_before = int(fh.read())

        # 401
        self.server.set_statuses(401)
        result = self._run_hook(payload)
        self.assertEqual(result.returncode, 0, "hook must exit 0 even on 401")
        with open(cursor_file, encoding="utf-8") as fh:
            self.assertEqual(int(fh.read()), offset_before)

        # 500
        self.server.set_statuses(500)
        result = self._run_hook(payload)
        self.assertEqual(result.returncode, 0, "hook must exit 0 even on 500")
        with open(cursor_file, encoding="utf-8") as fh:
            self.assertEqual(int(fh.read()), offset_before)

        # Unreachable server.
        env = dict(os.environ)
        env["HIVEMIND_API_URL"] = "http://127.0.0.1:1"  # nothing listens here
        env["HOME"] = self.cursor_home
        result = subprocess.run(
            [sys.executable, HOOK_SHIP],
            input=json.dumps(payload).encode(),
            env=env,
            capture_output=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0, "hook must exit 0 even when unreachable")
        with open(cursor_file, encoding="utf-8") as fh:
            self.assertEqual(int(fh.read()), offset_before)

        # Now succeed: the same batch (turn a1) must resend, not be lost.
        # (requests[] already holds the 401 and 500 attempts that reached
        # the stub; the unreachable-server attempt never did.)
        self.server.set_statuses(202)
        result = self._run_hook(payload)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.server.requests[-1]["turns"][0]["turn_id"], "a1")
        with open(cursor_file, encoding="utf-8") as fh:
            self.assertGreater(int(fh.read()), offset_before)

    def test_falls_back_to_env_when_stdin_has_no_payload(self):
        """No stdin payload (manual invocation): falls back to
        CLAUDE_CODE_SESSION_ID and CLAUDE_PROJECT_DIR / cwd mangling."""
        self._write_transcript(
            [
                {
                    "type": "user",
                    "uuid": "u1",
                    "message": {
                        "role": "user",
                        "content": [{"type": "text", "text": "hi"}],
                    },
                }
            ]
        )

        # Place the transcript where jsonl_path_for_session's mangling of
        # CLAUDE_PROJECT_DIR would find it.
        project_dir = "/proj.with.dots"
        mangled = "-proj-with-dots"
        claude_home = os.path.join(self.cursor_home, ".claude", "projects", mangled)
        os.makedirs(claude_home, exist_ok=True)
        fallback_path = os.path.join(claude_home, f"{self.session_id}.jsonl")
        os.replace(self.transcript_path, fallback_path)

        env = dict(os.environ)
        env["HIVEMIND_API_URL"] = self.server.url
        env["HOME"] = self.cursor_home
        env["CLAUDE_CODE_SESSION_ID"] = self.session_id
        env["CLAUDE_PROJECT_DIR"] = project_dir

        # No stdin content at all.
        result = subprocess.run(
            [sys.executable, HOOK_SHIP],
            input=b"",
            env=env,
            capture_output=True,
            timeout=15,
        )
        self.assertEqual(result.returncode, 0)

        cursor_file = os.path.join(
            self.cursor_home, ".hivemind", "cursors", f"{self.session_id}.offset"
        )
        self.assertTrue(
            os.path.exists(cursor_file),
            "env-var fallback must still locate and process the transcript",
        )


if __name__ == "__main__":
    unittest.main()
