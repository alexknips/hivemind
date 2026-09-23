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

    def test_mangles_every_non_alnum_character_not_just_slash(self):
        """Claude Code's real mangling replaces dots/underscores too, not just
        '/'. A naive '/' -> '-' replace misses cwds like gc's own agent
        worktrees (/home/ubuntu/gc/.gc/agents/mayor)."""
        path = jsonl_path_for_session(
            "session-abc", "/home/ubuntu/gc/.gc/agents/mayor"
        )
        home = os.path.expanduser("~")
        self.assertEqual(
            path,
            f"{home}/.claude/projects/-home-ubuntu-gc--gc-agents-mayor/session-abc.jsonl",
        )


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

    def test_typed_prompt_str_content_extracts_to_exact_text(self):
        """Claude Code stores a typed prompt as message.content = str."""
        prompt = "Go with option B, the shared Postgres cell."
        turn = _extract_turn(self._make_record("user", "user", prompt))
        self.assertIsNotNone(turn)
        self.assertEqual(turn["text"], prompt)
        self.assertEqual(turn["role"], "user")
        self.assertFalse(turn["truncated"])

    def test_str_content_keeps_multiline_prompt_intact(self):
        prompt = "Decision:\n- use Postgres\n- drop SQLite"
        turn = _extract_turn(self._make_record("user", "user", prompt))
        self.assertEqual(turn["text"], prompt)

    def test_blank_str_content_is_not_a_turn(self):
        self.assertIsNone(_extract_turn(self._make_record("user", "user", "   ")))

    def test_non_str_non_list_content_is_skipped_not_raised(self):
        """A malformed record must not raise: an exception in extraction would
        wedge the cursor on that line forever."""
        self.assertIsNone(_extract_turn(self._make_record("user", "user", None)))


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

    def _user_turns(self, count):
        return [
            {"type": "user", "uuid": f"new{i}", "message": {"role": "user", "content": [{"type": "text", "text": f"new turn {i}"}]}}
            for i in range(count)
        ]

    def _cursor_offset(self):
        with open(os.path.join(self.cursor_dir, f"{self.session_id}.offset")) as fh:
            return int(fh.read())

    def _init_cursor_at_eof(self):
        """Write one old record and run ship() so the cursor sits at EOF."""
        import core as core_module
        self._write_jsonl(
            self.jsonl_path,
            [{"type": "user", "uuid": "old0", "message": {"role": "user", "content": [{"type": "text", "text": "old"}]}}],
        )
        orig_post = core_module._post
        core_module._post = lambda url, key, env: None
        try:
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
        finally:
            core_module._post = orig_post

    def test_span_larger_than_max_batch_ships_every_turn_in_chunks(self):
        """A span of more than _MAX_BATCH_TURNS turns ships all of them, oldest
        first, split across batches with distinct ids; the cursor ends at EOF."""
        import core as core_module
        self._init_cursor_at_eof()
        posted = []
        orig_post = core_module._post
        core_module._post = lambda url, key, env: posted.append(env)

        try:
            self._append_jsonl(self.jsonl_path, self._user_turns(10))

            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")

            self.assertEqual([len(env["turns"]) for env in posted], [4, 4, 2])
            shipped_ids = [t["turn_id"] for env in posted for t in env["turns"]]
            self.assertEqual(shipped_ids, [f"new{i}" for i in range(10)])
            batch_ids = [env["batch_id"] for env in posted]
            self.assertEqual(len(set(batch_ids)), 3, "each chunk needs its own batch_id")
            self.assertEqual(self._cursor_offset(), os.path.getsize(self.jsonl_path))
        finally:
            core_module._post = orig_post

    def test_failed_post_mid_span_leaves_cursor_and_resends_whole_span(self):
        """If a later chunk fails, the cursor stays put so the next run resends
        every turn of the span rather than only its tail."""
        import core as core_module
        self._init_cursor_at_eof()
        offset_before = self._cursor_offset()
        self._append_jsonl(self.jsonl_path, self._user_turns(10))
        orig_post = core_module._post

        posted = []

        def _fail_on_second_chunk(url, key, env):
            if posted:
                raise OSError("network down")
            posted.append(env)

        core_module._post = _fail_on_second_chunk
        try:
            # ship() must swallow the error.
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            self.assertEqual(len(posted), 1, "first chunk posted before the failure")
            self.assertEqual(self._cursor_offset(), offset_before)

            # Recovery: the retry ships all 10 turns and then advances.
            retried = []
            core_module._post = lambda url, key, env: retried.append(env)
            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
            shipped_ids = [t["turn_id"] for env in retried for t in env["turns"]]
            self.assertEqual(shipped_ids, [f"new{i}" for i in range(10)])
            self.assertEqual(self._cursor_offset(), os.path.getsize(self.jsonl_path))
        finally:
            core_module._post = orig_post

    def test_typed_prompt_string_content_ships_verbatim(self):
        """A typed user prompt (content is a str) reaches the ingest payload as
        the exact original text, not one character per token."""
        import core as core_module
        self._init_cursor_at_eof()
        posted = []
        orig_post = core_module._post
        core_module._post = lambda url, key, env: posted.append(env)

        try:
            prompt = "Go with option B, the shared Postgres cell."
            self._append_jsonl(
                self.jsonl_path,
                [{"type": "user", "uuid": "u1", "message": {"role": "user", "content": prompt}}],
            )

            ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")

            self.assertEqual(len(posted), 1)
            self.assertEqual(posted[0]["turns"][0]["text"], prompt)
        finally:
            core_module._post = orig_post

    def test_cursor_does_not_advance_when_post_fails(self):
        """A failed POST must not lose the batch: the cursor stays put so the
        next run resends the same turns instead of silently dropping them."""
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
        self.assertEqual(offset_after, offset_before, "cursor must not advance when POST fails")

        # Next run resends: same new turn arrives again, not skipped.
        core_module._post = orig_post
        posted = []
        core_module._post = lambda url, key, env: posted.append(env)
        ship(self.session_id, self.jsonl_path, "http://localhost:8080", "")
        self.assertEqual(len(posted), 1)
        self.assertEqual(posted[0]["turns"][0]["turn_id"], "u1")

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
