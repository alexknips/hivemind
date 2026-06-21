#!/usr/bin/env python3
"""Unit tests for capture/core.py."""

import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from core import (
    _extract_turn,
    _write_cursor,
    cursor_path_for_session,
    jsonl_path_for_session,
    ship,
)


class TestJsonlPathForSession(unittest.TestCase):
    def test_derives_path_from_project_dir(self):
        path = jsonl_path_for_session("session-abc", "/data/projects/hivemind")
        home = os.path.expanduser("~")
        self.assertEqual(
            path,
            f"{home}/.claude/projects/-data-projects-hivemind/session-abc.jsonl",
        )

    def test_uses_cwd_when_project_dir_omitted(self):
        original_cwd = os.getcwd()
        try:
            os.chdir(tempfile.gettempdir())
            path = jsonl_path_for_session("s1")
            self.assertIn("s1.jsonl", path)
        finally:
            os.chdir(original_cwd)


class TestExtractTurn(unittest.TestCase):
    def _make_record(self, type_, role, content):
        return {
            "type": type_,
            "uuid": "uuid-1",
            "message": {"role": role, "content": content},
        }

    def test_extracts_user_text_turn(self):
        record = self._make_record(
            "user", "user", [{"type": "text", "text": "Should we use REST or RPC?"}]
        )
        turn = _extract_turn(record)
        self.assertIsNotNone(turn)
        self.assertEqual(turn["role"], "user")
        self.assertEqual(turn["text"], "Should we use REST or RPC?")
        self.assertFalse(turn["truncated"])

    def test_extracts_assistant_text_turn(self):
        record = self._make_record(
            "assistant",
            "assistant",
            [{"type": "text", "text": "REST is better for ergonomics."}],
        )
        turn = _extract_turn(record)
        self.assertIsNotNone(turn)
        self.assertEqual(turn["role"], "assistant")
        self.assertEqual(turn["text"], "REST is better for ergonomics.")

    def test_skips_non_user_assistant_types(self):
        record = {"type": "file-history-snapshot", "uuid": "u", "message": {}}
        self.assertIsNone(_extract_turn(record))

    def test_summarises_tool_use(self):
        record = self._make_record(
            "assistant",
            "assistant",
            [
                {"type": "text", "text": "Running a check."},
                {"type": "tool_use", "name": "Bash", "id": "t1", "input": {}},
            ],
        )
        turn = _extract_turn(record)
        self.assertIsNotNone(turn)
        self.assertIn("[Tool: Bash]", turn["text"])
        self.assertTrue(turn["truncated"])

    def test_skips_thinking_blocks(self):
        record = self._make_record(
            "assistant",
            "assistant",
            [
                {"type": "thinking", "thinking": "private reasoning"},
                {"type": "text", "text": "My answer."},
            ],
        )
        turn = _extract_turn(record)
        self.assertIsNotNone(turn)
        self.assertNotIn("private reasoning", turn["text"])
        self.assertEqual(turn["text"], "My answer.")

    def test_truncates_long_text(self):
        long_text = "x" * 3000
        record = self._make_record(
            "user", "user", [{"type": "text", "text": long_text}]
        )
        turn = _extract_turn(record)
        self.assertTrue(turn["truncated"])
        self.assertLessEqual(len(turn["text"]), 2010)

    def test_returns_none_for_empty_text(self):
        record = self._make_record(
            "assistant", "assistant", [{"type": "thinking", "thinking": "nope"}]
        )
        self.assertIsNone(_extract_turn(record))

    def test_handles_plain_string_content(self):
        record = self._make_record("user", "user", ["Hello from a plain string"])
        turn = _extract_turn(record)
        self.assertIsNotNone(turn)
        self.assertIn("Hello from a plain string", turn["text"])


class TestCursorFlow(unittest.TestCase):
    def _write_jsonl(self, path, records):
        with open(path, "w", encoding="utf-8") as fh:
            for record in records:
                fh.write(json.dumps(record) + "\n")

    def _append_jsonl(self, path, records):
        with open(path, "a", encoding="utf-8") as fh:
            for record in records:
                fh.write(json.dumps(record) + "\n")

    def setUp(self):
        self.tmpdir = tempfile.mkdtemp()
        self.session_id = "test-session-cursor"
        self.jsonl_path = os.path.join(self.tmpdir, f"{self.session_id}.jsonl")
        self.cursor_dir = os.path.join(self.tmpdir, "cursors")

        # Patch cursor_path to use a temp directory.
        import core as core_module
        self._orig_cursor_path = core_module.cursor_path_for_session

        def patched_cursor_path(sid):
            return os.path.join(self.cursor_dir, f"{sid}.offset")

        core_module.cursor_path_for_session = patched_cursor_path

    def tearDown(self):
        import core as core_module
        core_module.cursor_path_for_session = self._orig_cursor_path
        import shutil
        shutil.rmtree(self.tmpdir, ignore_errors=True)

    def test_first_run_sets_cursor_to_eof_and_ships_nothing(self):
        self._write_jsonl(
            self.jsonl_path,
            [
                {"type": "user", "uuid": "u1", "message": {"role": "user", "content": [{"type": "text", "text": "old turn"}]}}
            ],
        )

        posted = []

        import core as core_module
        orig_post = core_module._post
        core_module._post = lambda url, key, env: posted.append(env)

        try:
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            self.assertEqual(posted, [], "first run must not ship existing content")

            # Cursor file should exist now.
            cursor_file = os.path.join(self.cursor_dir, f"{self.session_id}.offset")
            self.assertTrue(os.path.exists(cursor_file))
            with open(cursor_file, encoding="utf-8") as fh:
                offset = int(fh.read())
            file_size = os.path.getsize(self.jsonl_path)
            self.assertEqual(offset, file_size)
        finally:
            core_module._post = orig_post

    def test_second_run_ships_only_new_turns(self):
        # Write initial content and init cursor.
        self._write_jsonl(
            self.jsonl_path,
            [
                {"type": "user", "uuid": "old", "message": {"role": "user", "content": [{"type": "text", "text": "old"}]}}
            ],
        )

        import core as core_module
        posted = []
        orig_post = core_module._post
        core_module._post = lambda url, key, env: posted.append(env)

        try:
            # First call: init cursor to EOF.
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            self.assertEqual(posted, [])

            # Append a new turn.
            self._append_jsonl(
                self.jsonl_path,
                [
                    {"type": "assistant", "uuid": "new1", "message": {"role": "assistant", "content": [{"type": "text", "text": "new answer"}]}}
                ],
            )

            # Second call: should ship only the new turn.
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            self.assertEqual(len(posted), 1)
            self.assertEqual(len(posted[0]["turns"]), 1)
            self.assertEqual(posted[0]["turns"][0]["turn_id"], "new1")
            self.assertEqual(posted[0]["session_id"], self.session_id)
        finally:
            core_module._post = orig_post

    def test_batch_truncates_to_last_max_batch_turns(self):
        """When more new turns exist than _MAX_BATCH_TURNS, ship only the last N."""
        import core as core_module
        posted = []
        orig_post = core_module._post
        core_module._post = lambda url, key, env: posted.append(env)

        try:
            # Write some existing content, then init cursor to EOF.
            self._write_jsonl(
                self.jsonl_path,
                [{"type": "user", "uuid": "old0", "message": {"role": "user", "content": [{"type": "text", "text": "old"}]}}],
            )
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            self.assertEqual(posted, [])

            # Append 5 new turns (exceeds _MAX_BATCH_TURNS=4).
            new_turns = [
                {"type": "user", "uuid": f"new{i}", "message": {"role": "user", "content": [{"type": "text", "text": f"new turn {i}"}]}}
                for i in range(5)
            ]
            self._append_jsonl(self.jsonl_path, new_turns)

            # Second run: must ship exactly _MAX_BATCH_TURNS=4, the last four.
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            self.assertEqual(len(posted), 1)
            batch_turns = posted[0]["turns"]
            self.assertEqual(len(batch_turns), 4, f"expected 4 turns (max batch), got {len(batch_turns)}")
            turn_ids = [t["turn_id"] for t in batch_turns]
            self.assertNotIn("new0", turn_ids, "oldest turn must be dropped")
            self.assertIn("new4", turn_ids, "newest turn must be included")
        finally:
            core_module._post = orig_post

    def test_cursor_advances_even_when_post_fails(self):
        self._write_jsonl(
            self.jsonl_path,
            [
                {"type": "user", "uuid": "u0", "message": {"role": "user", "content": [{"type": "text", "text": "turn zero"}]}}
            ],
        )

        import core as core_module

        # Init cursor.
        orig_post = core_module._post
        core_module._post = lambda url, key, env: None
        ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")

        # Append new turn.
        self._append_jsonl(
            self.jsonl_path,
            [{"type": "user", "uuid": "u1", "message": {"role": "user", "content": [{"type": "text", "text": "new"}]}}],
        )

        # Make POST raise.
        def _fail(*_args, **_kwargs):
            raise OSError("network down")

        core_module._post = _fail

        cursor_file = os.path.join(self.cursor_dir, f"{self.session_id}.offset")
        with open(cursor_file) as fh:
            offset_before = int(fh.read())

        # ship() should not raise; it prints to stderr.
        ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")

        with open(cursor_file) as fh:
            offset_after = int(fh.read())
        self.assertGreater(offset_after, offset_before, "cursor must advance despite POST failure")

        core_module._post = orig_post


class TestPostHttpError(unittest.TestCase):
    """_post should surface HTTP error status and response body in the exception."""

    def test_http_error_message_includes_status_and_body(self):
        import io
        import urllib.error
        import urllib.request

        import core as core_module

        fake_response_body = b'{"error":{"code":"validation_error","message":"batch_id must not be empty"}}'

        original_urlopen = urllib.request.urlopen

        def mock_urlopen(req, timeout=None):
            raise urllib.error.HTTPError(
                req.full_url,
                400,
                "Bad Request",
                {},
                io.BytesIO(fake_response_body),
            )

        urllib.request.urlopen = mock_urlopen
        try:
            with self.assertRaises(RuntimeError) as cm:
                core_module._post("http://localhost:8080", "", {"batch_id": ""})
            msg = str(cm.exception)
            self.assertIn("400", msg)
            self.assertIn("validation_error", msg)
        finally:
            urllib.request.urlopen = original_urlopen


if __name__ == "__main__":
    unittest.main()
