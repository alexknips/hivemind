#!/usr/bin/env python3
"""Claude Code hooks for AskUserQuestion (hivemind-bbnw.5).

An agent's AskUserQuestion is an ask that somebody performed, at a time. This records it:

  pre   (PreToolUse)   -> `request_decision` per question: the ask, at the moment the tool is
                          called. Nothing is inferred and nothing is back-dated.
  post  (PostToolUse)  -> `capture_decision` per answered question: the human's answer as a
                          decision that answers the same question. Decider = the human, options =
                          the offered ones, chosen = the picked one, the human's own words (an
                          "Other" answer) quoted verbatim, and the note they added to their pick
                          (the tool's `annotations[question].notes`) as the rationale, word for
                          word. Only with neither a note nor own words does the rationale say no
                          reasons were given.

The two hooks share no state. The link between an ask and its answer is the question itself: the
ask and the decision name the same question text, so they resolve to one Question node, the way
`hivemind ask` and `capture --question` do. `why` then shows asked_at beside the answer's time, and
the request stops being listed as waiting.

A question the human never answers (declined, session closed) has no PostToolUse, so its request
stays waiting. When the ask itself was never recorded (the plugin was installed mid-question, the
write failed) the answer is still captured and `why` shows no asked_at, which is the truth.

The answer is filed under one topic key: the slug of the question's header. A registered project
accepts only topic keys it declared, so when the write refuses that key as undeclared the answer is
written once more under the fixed key `claude-code-question`, declaring only that key. A project's
vocabulary therefore grows by at most one key from these hooks, never one per question. A personal
or unregistered checkout has no vocabulary: the first write succeeds and nothing is declared.

Writes go to a local ledger through `hivemind mcp` (stdio), or, when HIVEMIND_API_URL is set, to
that server's POST /mcp with HIVEMIND_API_KEY as the bearer token. Both take the same tool calls.

This must never get in the agent's way: every failure is logged (stderr and hook.log in the state
directory) and the hook still exits 0 with nothing on stdout. Stdlib only.

Environment:
  HIVEMIND_ASK_HOOK_DISABLE  any value: do nothing.
  HIVEMIND_API_URL / HIVEMIND_API_KEY  write to a server instead of a local ledger.
  HIVEMIND_DIR / CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR  local ledger directory; default is
                             <repo root>/hivemind, the same rule as capture.sh.
  HIVEMIND_CAPTURE_BIN       the hivemind binary; default is `hivemind` on PATH.
  HIVEMIND_HUMAN_ACTOR       who answers: human:<name>; default is human:<git user.email>.
  HIVEMIND_PROJECT           file the answers under this registered project.
"""

from __future__ import annotations

import getpass
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

TOOL_NAME = "AskUserQuestion"
CLIENT_NAME = "hivemind-ask-hook"
RPC_TIMEOUT_S = 15
LOG_MAX_BYTES = 1_000_000

# The write layer's limits on what a decision may carry (src/commands/mod.rs).
MAX_LABEL_CHARS = 80
MAX_TITLE_CHARS = 120

OWN_WORDS_LABEL = "Other (own words)"
COMBINED_DESCRIPTION = (
    "The person's answer as given: several offered options together, or their own words."
)

# A person's reasoning is theirs to state, so this hook never writes any for them. The rationale is
# the note they added to their pick, word for word. Only when there is neither a note nor words of
# their own does it say, in a constant sentence, that no reasons were given. The sentences carry no
# question or label text (those can trip the write layer's readable-rationale check); the question,
# the options and the person's own words are recorded in their own fields.
RATIONALE_PICKED = (
    "A person chose this answer from the options an AI agent offered in a Claude Code question; "
    "no reasons were given with the answer."
)
RATIONALE_OWN_WORDS = (
    "A person answered an AI agent's Claude Code question in their own words, quoted with this "
    "decision."
)
# A note the write layer refuses as a rationale (too short to read on its own) is kept in the
# quote instead, and the rationale says so.
RATIONALE_NOTE_ATTACHED = (
    "A person answered an AI agent's Claude Code question and added a note to their answer, "
    "quoted with this decision."
)
NOTE_QUOTE_PREFIX = "Note: "

# The topic key an answer is filed under when its project has not declared the header's key, and
# the words of the write layer's refusal for a key a registered project never declared ("topic
# `x` is not declared for project p (declared: ...)", src/commands/mod.rs). Only this refusal
# sends the answer to the fixed key; any other refusal is not retried.
FALLBACK_TOPIC = "claude-code-question"
UNDECLARED_TOPIC_REFUSAL = "not declared for project"


class HookError(Exception):
    """A write that did not happen, with the reason to log."""


# --------------------------------------------------------------------------------------------
# Transports: one MCP `tools/call` at a time, over a stdio child or HTTP.
# --------------------------------------------------------------------------------------------


def unwrap(response: Dict[str, Any]) -> Dict[str, Any]:
    """The structured reply of a `tools/call` response, or a HookError saying why not."""
    error = response.get("error")
    if error:
        raise HookError(f"{error.get('code')}: {error.get('message')}")
    result = response.get("result") or {}
    text = "".join(
        part.get("text", "") for part in result.get("content", []) if isinstance(part, dict)
    )
    if result.get("isError"):
        raise HookError(text or "the tool call failed")
    structured = result.get("structuredContent")
    if isinstance(structured, dict):
        return structured
    try:
        parsed = json.loads(text)
    except ValueError:
        raise HookError(f"unreadable tool reply: {text[:200]}")
    if not isinstance(parsed, dict):
        raise HookError(f"unexpected tool reply: {text[:200]}")
    return parsed


def rpc_request(request_id: int, name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }


class StdioTransport:
    """`hivemind mcp` as a child process, one per hook run."""

    def __init__(self, binary: str, hivemind_dir: str, cwd: str, env: Dict[str, str]) -> None:
        self.command = [
            binary,
            "--hivemind-dir",
            hivemind_dir,
            "mcp",
            "--agent-tool",
            "claude",
            "--project-from-context",
        ]
        self.cwd = cwd
        self.env = env
        self.proc: Optional["subprocess.Popen[str]"] = None
        self.next_id = 0

    def __enter__(self) -> "StdioTransport":
        try:
            self.proc = subprocess.Popen(
                self.command,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                cwd=self.cwd,
                env=self.env,
                text=True,
            )
        except OSError as exc:
            raise HookError(f"could not start {self.command[0]}: {exc}")
        return self

    def call(self, name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        proc = self.proc
        if proc is None or proc.stdin is None or proc.stdout is None:
            raise HookError("hivemind mcp is not running")
        self.next_id += 1
        try:
            proc.stdin.write(json.dumps(rpc_request(self.next_id, name, arguments)) + "\n")
            proc.stdin.flush()
        except OSError as exc:
            raise HookError(f"hivemind mcp closed its input: {exc}")
        line = proc.stdout.readline()
        if not line:
            raise HookError("hivemind mcp exited without answering")
        try:
            return unwrap(json.loads(line))
        except ValueError:
            raise HookError(f"hivemind mcp sent something that is not JSON: {line[:200]}")

    def __exit__(self, *_exc: object) -> None:
        proc = self.proc
        if proc is None:
            return
        try:
            if proc.stdin is not None:
                proc.stdin.close()
            proc.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            proc.kill()


class HttpTransport:
    """POST /mcp on a HiveMind server; the token, not this hook, decides who is writing."""

    def __init__(self, base_url: str, api_key: str) -> None:
        self.url = base_url.rstrip("/") + "/mcp"
        self.api_key = api_key
        self.next_id = 0

    def __enter__(self) -> "HttpTransport":
        return self

    def call(self, name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        self.next_id += 1
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "User-Agent": CLIENT_NAME,
        }
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        body = json.dumps(rpc_request(self.next_id, name, arguments)).encode("utf-8")
        request = urllib.request.Request(self.url, data=body, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=RPC_TIMEOUT_S) as response:
                return unwrap(json.load(response))
        except urllib.error.HTTPError as exc:
            raise HookError(f"{self.url} answered HTTP {exc.code}")
        except (urllib.error.URLError, OSError, ValueError) as exc:
            raise HookError(f"{self.url} did not answer: {exc}")

    def __exit__(self, *_exc: object) -> None:
        return None


Transport = Any  # StdioTransport | HttpTransport


def git_output(cwd: str, *args: str) -> str:
    try:
        done = subprocess.run(
            ["git", *args],
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return done.stdout.strip() if done.returncode == 0 else ""


def ledger_dir(cwd: str) -> str:
    """The local ledger: the same rule as capture.sh, made absolute."""
    explicit = os.environ.get("HIVEMIND_DIR") or os.environ.get("CLAUDE_PLUGIN_OPTION_HIVEMIND_DIR")
    if explicit:
        return str(Path(cwd, explicit).resolve())
    common = git_output(cwd, "rev-parse", "--path-format=absolute", "--git-common-dir")
    root = str(Path(common).parent) if common else (os.environ.get("CLAUDE_PROJECT_DIR") or cwd)
    return str(Path(root, "hivemind"))


def open_transport(cwd: str, session_id: str) -> Transport:
    url = os.environ.get("HIVEMIND_API_URL", "").strip()
    if url:
        return HttpTransport(url, os.environ.get("HIVEMIND_API_KEY", "").strip())
    binary = os.environ.get("HIVEMIND_CAPTURE_BIN") or shutil.which("hivemind")
    if not binary:
        raise HookError("the hivemind CLI was not found: install it, or set HIVEMIND_CAPTURE_BIN")
    env = dict(os.environ)
    # The actor is agent:claude:<name>. The stable Gas City slot (GC_AGENT/GC_ALIAS) wins inside
    # the server; the session id is the fallback, so both hooks write as the same actor.
    if session_id:
        env.setdefault("CLAUDE_CODE_SESSION_ID", session_id)
    return StdioTransport(binary, ledger_dir(cwd), cwd, env)


def log(message: str) -> None:
    """stderr for a verbose transcript, hook.log for later; the hook still exits 0."""
    line = f"{time.strftime('%Y-%m-%dT%H:%M:%S%z')} {message}\n"
    sys.stderr.write(f"hivemind ask hook: {line}")
    try:
        directory = log_dir()
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        path = directory / "hook.log"
        if path.exists() and path.stat().st_size > LOG_MAX_BYTES:
            path.replace(directory / "hook.log.1")
        # The log names question text, so it is the owner's alone.
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        with os.fdopen(descriptor, "a", encoding="utf-8") as handle:
            handle.write(line)
    except (OSError, RuntimeError):
        pass


def log_dir() -> Path:
    """The plugin's own data directory, else the user's state directory (never a shared /tmp)."""
    plugin_data = os.environ.get("CLAUDE_PLUGIN_DATA")
    if plugin_data:
        return Path(plugin_data) / "ask-hook"
    state_home = os.environ.get("XDG_STATE_HOME") or str(Path.home() / ".local" / "state")
    return Path(state_home) / "hivemind-ask-hook"


# --------------------------------------------------------------------------------------------
# Reading the tool call.
# --------------------------------------------------------------------------------------------


def one_line(text: Any) -> str:
    return " ".join(str(text or "").split())


def questions_of(payload: Dict[str, Any]) -> List[Dict[str, Any]]:
    tool_input = payload.get("tool_input")
    questions = tool_input.get("questions") if isinstance(tool_input, dict) else None
    if not isinstance(questions, list):
        return []
    return [q for q in questions if isinstance(q, dict) and one_line(q.get("question"))]


def answers_of(payload: Dict[str, Any]) -> Dict[str, str]:
    """{question text: the answer as the tool reported it}, keyed on one-line question text."""
    # The tool's response carries the answers; the call's own input repeats them once answered.
    answers = None
    for source in (payload.get("tool_response"), payload.get("tool_input")):
        found = source.get("answers") if isinstance(source, dict) else None
        if isinstance(found, dict) and found:
            answers = found
            break
    if not isinstance(answers, dict):
        return {}
    return {one_line(q): str(a) for q, a in answers.items() if one_line(q) and str(a).strip()}


def notes_of(payload: Dict[str, Any]) -> Dict[str, str]:
    """{question text: the note the person added to their pick}, keyed on one-line question text.

    The tool reports `annotations` beside `answers`, keyed by question: `{"<question>": {"notes":
    "..."}}`. A note is the person's free text about their pick, kept as written.
    """
    notes: Dict[str, str] = {}
    for source in (payload.get("tool_input"), payload.get("tool_response")):
        annotations = source.get("annotations") if isinstance(source, dict) else None
        if not isinstance(annotations, dict):
            continue
        for question, annotation in annotations.items():
            note = annotation.get("notes") if isinstance(annotation, dict) else None
            if one_line(question) and isinstance(note, str) and note.strip():
                notes[one_line(question)] = note.strip()
    return notes


def offered_labels(question: Dict[str, Any]) -> List[Tuple[str, str]]:
    """[(label, description)] in the order offered."""
    offered = []
    for option in question.get("options") or []:
        if isinstance(option, dict) and one_line(option.get("label")):
            offered.append((one_line(option["label"]), one_line(option.get("description"))))
    return offered


def split_answer(answer: str, labels: List[str], multi: bool) -> Tuple[List[str], Optional[str]]:
    """(offered labels the person picked, their own words if any).

    The tool reports one string: the label for a single choice, the labels joined with ", " for a
    multi-select, and whatever they typed for "Other". Labels may contain ", " themselves, so a
    multi-select answer is matched against the offered labels (longest first) rather than split.
    """
    if not multi:
        return ([one_line(answer)], None) if one_line(answer) in labels else ([], answer)
    picked: List[str] = []
    rest = answer
    by_length = sorted(labels, key=len, reverse=True)
    while rest:
        for label in by_length:
            if label in picked:
                continue
            if rest == label:
                picked.append(label)
                rest = ""
                break
            if rest.startswith(label + ", "):
                picked.append(label)
                rest = rest[len(label) + 2 :]
                break
        else:
            break
    ordered = [label for label in labels if label in picked]
    return ordered, (rest or None)


def clip(text: str, limit: int) -> str:
    return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"


def slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")[:40]


def human_actor(cwd: str) -> str:
    configured = os.environ.get("HIVEMIND_HUMAN_ACTOR", "").strip()
    if configured.startswith("human:"):
        return configured
    raw = (
        configured
        or git_output(cwd, "config", "user.email")
        or git_output(cwd, "config", "user.name")
        or getpass.getuser()
    )
    name = re.sub(r"[^a-z0-9_.@-]", "-", raw.lower()).strip("-")
    return f"human:{name or 'local-user'}"


def answer_arguments(
    question: Dict[str, Any],
    answer: str,
    human: str,
    note: Optional[str] = None,
    note_as_quote: bool = False,
    fallback_topic: bool = False,
) -> Dict[str, Any]:
    """The `capture_decision` call for one answered question.

    The rationale is the person's `note` on their pick, word for word. With `note_as_quote` (the
    write layer refused the note as a rationale) the note rides in the quote instead and the
    rationale says a note is attached. With neither a note nor own words, the constant sentence
    says no reasons were given.

    The topic key is the slug of the header. With `fallback_topic` (the project has not declared
    it) the answer goes under FALLBACK_TOPIC and the call declares that one key.
    """
    text = one_line(question.get("question"))
    offered = offered_labels(question)
    picked, own_words = split_answer(
        answer, [label for label, _ in offered], bool(question.get("multiSelect"))
    )

    offered_options: List[Dict[str, str]] = []
    for label, description in offered:
        entry = {"label": clip(label, MAX_LABEL_CHARS)}
        if description:
            entry["description"] = description
        if all(o["label"] != entry["label"] for o in offered_options):
            offered_options.append(entry)
    # One decision has one chosen option: several picks, or the person's own words, become one
    # option saying so. What was picked into it is not also listed as an option the person
    # turned down, so only the offered options they did not pick stay beside it.
    names = [clip(label, MAX_LABEL_CHARS) for label in picked]
    if own_words:
        names.append(OWN_WORDS_LABEL)
    chosen = clip(" + ".join(names), MAX_LABEL_CHARS)
    if any(o["label"] == chosen for o in offered_options):
        options = offered_options
    else:
        options = [o for o in offered_options if o["label"] not in names]
        options.append({"label": chosen, "description": COMBINED_DESCRIPTION})

    header = one_line(question.get("header")) or clip(text, 60)
    if note and not note_as_quote:
        rationale = note
    elif note:
        rationale = RATIONALE_NOTE_ATTACHED
    elif own_words:
        rationale = RATIONALE_OWN_WORDS
    else:
        rationale = RATIONALE_PICKED

    title = re.sub(r"[.!?]+(?=\s|$)", "", f"{header}: {chosen}")
    arguments: Dict[str, Any] = {
        "title": clip(title, MAX_TITLE_CHARS),
        "rationale": rationale,
        "topic_keys": [FALLBACK_TOPIC if fallback_topic else slug(header) or FALLBACK_TOPIC],
        "options": options,
        "chosen_option_label": chosen,
        "decided_by": human,
        "question": text,
        # The write layer asks what every decision rests on. Nothing was stated with the answer,
        # so it is recorded as what it is: a bet with nothing declared.
        "grounding": [{"kind": "bet"}],
    }
    if fallback_topic:
        arguments["declare_topics"] = [FALLBACK_TOPIC]
    quoted = [own_words] if own_words else []
    if note and note_as_quote:
        quoted.append(NOTE_QUOTE_PREFIX + note if own_words else note)
    if quoted:
        arguments["quote"] = "\n\n".join(quoted)
    project = os.environ.get("HIVEMIND_PROJECT", "").strip()
    if project:
        arguments["project"] = project
    return arguments


# --------------------------------------------------------------------------------------------
# The two hooks.
# --------------------------------------------------------------------------------------------


def session_cwd(payload: Dict[str, Any]) -> str:
    cwd = payload.get("cwd")
    return cwd if isinstance(cwd, str) and os.path.isdir(cwd) else os.getcwd()


def record_asks(payload: Dict[str, Any]) -> None:
    questions = questions_of(payload)
    if not questions:
        return
    with open_transport(session_cwd(payload), str(payload.get("session_id") or "")) as transport:
        for question in questions:
            text = one_line(question["question"])
            try:
                transport.call("request_decision", {"text": text})
            except HookError as exc:
                log(f"pre: could not record the ask {text[:60]!r}: {exc}")


def capture_answer(
    transport: Transport, question: Dict[str, Any], answer: str, human: str, note: Optional[str]
) -> None:
    """Write one answer, again after a refusal that has a known way out, once per kind:

    - the project never declared the header's topic key: write it under FALLBACK_TOPIC, declaring
      only that key;
    - the readable-rationale check refused the person's note (too short to stand alone): write it
      with the note in the quote.

    Every other refusal, and any transport failure, is raised and not retried: a refusal wrote
    nothing, but a failure may have reached the ledger already and a second write would duplicate
    the decision. The two ways out are independent, so at most two retries happen, each logged.
    """
    text = one_line(question.get("question"))[:60]
    note_as_quote = False
    fallback_topic = False
    while True:
        arguments = answer_arguments(
            question,
            answer,
            human,
            note,
            note_as_quote=note_as_quote,
            fallback_topic=fallback_topic,
        )
        try:
            transport.call("capture_decision", arguments)
        except HookError as exc:
            reason = str(exc)
            # The topic refusal is told apart first: it names the header's key, which may itself
            # contain the word "rationale".
            if not fallback_topic and UNDECLARED_TOPIC_REFUSAL in reason:
                log(
                    f"post: the project does not declare topic {arguments['topic_keys'][0]!r} "
                    f"({reason}); writing the answer to {text!r} again under {FALLBACK_TOPIC!r}"
                )
                fallback_topic = True
            elif note and not note_as_quote and "rationale" in reason:
                note_as_quote = True
            else:
                raise
        else:
            if fallback_topic:
                log(f"post: recorded the answer to {text!r} under {FALLBACK_TOPIC!r}")
            return


def record_answers(payload: Dict[str, Any]) -> None:
    answers = answers_of(payload)
    questions = questions_of(payload)
    if not answers or not questions:
        return
    notes = notes_of(payload)
    cwd = session_cwd(payload)
    human = human_actor(cwd)
    with open_transport(cwd, str(payload.get("session_id") or "")) as transport:
        for question in questions:
            text = one_line(question["question"])
            answer = answers.get(text)
            if not answer:
                continue
            try:
                capture_answer(transport, question, answer.strip(), human, notes.get(text))
            except HookError as exc:
                log(f"post: could not record the answer to {text[:60]!r}: {exc}")


def main(argv: List[str]) -> int:
    if os.environ.get("HIVEMIND_ASK_HOOK_DISABLE"):
        return 0
    phase = argv[1] if len(argv) > 1 else ""
    try:
        payload = json.load(sys.stdin)
        if not isinstance(payload, dict) or payload.get("tool_name") != TOOL_NAME:
            return 0
        if phase == "pre":
            record_asks(payload)
        elif phase == "post":
            record_answers(payload)
        else:
            log(f"unknown phase {phase!r}: expected pre or post")
    except Exception as exc:  # never get in the agent's way
        log(f"{phase}: {exc}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
