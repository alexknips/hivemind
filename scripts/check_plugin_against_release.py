#!/usr/bin/env python3
"""Run the plugins' capture and MCP invocations against the latest RELEASE `hivemind` binary.

The marketplace serves the plugins from master, but people install the latest release binary, so
every plugin change runs ahead of the CLI under it. A flag or verb only master has breaks a
stranger's install while every master-built test stays green (hivemind-cxqd). This is that check:

  * both MCP configs (Claude Code's `.mcp.json`, the Codex bundle's `.codex-plugin/mcp.json`) are
    started the way a client starts them, handshake, and take one grounded capture that lands
    attributed to their own tool (agent:claude / agent:codex);
  * `capture.sh` writes a decision, evidence and a hypothesis, under Claude Code's and Codex's
    environment, and `query-decisions.sh` reads them back;
  * the Codex skill's direct CLI form works as the skill says to use it on an older CLI;
  * the AskUserQuestion hooks never get in the way and still record the human's answer;
  * the SessionStart directive names only tools the release's MCP server serves;
  * the hivemind-context scripts run, and a verb the release lacks is refused with that said.

Everything runs under a throwaway HOME, with a scrubbed environment and the release binary alone on
PATH. Python standard library only.

  scripts/check_plugin_against_release.py                 download the latest release (needs gh)
  scripts/check_plugin_against_release.py --bin PATH      use this binary as "the release"
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
from pathlib import Path
from typing import Any, Dict, List, Optional

ROOT = Path(__file__).resolve().parent.parent
CAPTURE = ROOT / "plugins" / "hivemind-capture"
CONTEXT = ROOT / "plugins" / "hivemind-context"
HOOK_FIXTURES = ROOT / "tests" / "fixtures" / "claude_code" / "ask_user_question"

RPC_TIMEOUT_S = 60

# What each tool puts in the environment of the processes it starts. Claude Code names the project
# folder to its plugins; Codex does not, and CLAUDE_PROJECT_DIR alone makes capture.sh say claude.
SESSIONS = {
    "claude": {"CLAUDE_CODE_SESSION_ID": "check-claude-session"},
    "codex": {"CODEX_THREAD_ID": "check-codex-thread"},
}

FAILURES: List[str] = []


def check(name: str, ok: bool, detail: str = "") -> bool:
    print(f"{'PASS' if ok else 'FAIL'}  {name}")
    if not ok:
        FAILURES.append(name)
        if detail.strip():
            for line in detail.strip().splitlines()[-12:]:
                print(f"        {line}")
    return ok


# --------------------------------------------------------------------------------------------
# The release binary
# --------------------------------------------------------------------------------------------


def release_asset() -> str:
    system, machine = platform.system(), platform.machine().lower()
    arch = "arm64" if machine in ("arm64", "aarch64") else "x86_64"
    if system == "Linux":
        return f"hivemind-linux-{arch}.tar.gz"
    if system == "Darwin" and arch == "arm64":
        return "hivemind-macos-arm64.tar.gz"
    raise SystemExit(f"no release binary for {system} {machine}; pass --bin")


def download_latest_release(repo: Optional[str], into: Path) -> Path:
    asset = release_asset()
    command = ["gh", "release", "download", "--pattern", f"{asset}*", "--dir", str(into)]
    if repo:
        command += ["--repo", repo]
    subprocess.run(command, check=True, timeout=300)
    archive = into / asset
    expected = (into / f"{asset}.sha256").read_text().split()[0]
    actual = hashlib.sha256(archive.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"{asset}: sha256 {actual} does not match the published {expected}")
    with tarfile.open(archive) as tar:
        if hasattr(tarfile, "data_filter"):
            tar.extractall(into, filter="data")
        else:
            tar.extractall(into)
    binary = into / "hivemind"
    binary.chmod(0o755)
    return binary


# --------------------------------------------------------------------------------------------
# A throwaway world: HOME, project folder, scrubbed environment
# --------------------------------------------------------------------------------------------


class World:
    def __init__(self, root: Path, binary: Path) -> None:
        self.root = root
        self.home = root / "home"
        self.bin = root / "bin"
        self.home.mkdir()
        self.bin.mkdir()
        (self.bin / "hivemind").symlink_to(binary.resolve())
        self.n = 0

    def project(self, label: str) -> Path:
        """A fresh project folder with its own ledger directory: `<project>/hivemind`."""
        self.n += 1
        folder = self.root / f"{self.n:02d}-{label}"
        folder.mkdir()
        return folder

    def env(self, project: Path, tool: Optional[str] = None, **extra: str) -> Dict[str, str]:
        """What `tool` hands a plugin process: nothing of this machine's agent identity."""
        env = {
            "HOME": str(self.home),
            "PATH": f"{self.bin}:/usr/local/bin:/usr/bin:/bin",
            "LANG": "C.UTF-8",
            "TMPDIR": str(self.root),
            "XDG_STATE_HOME": str(self.home / "state"),
        }
        if tool:
            env.update(SESSIONS[tool])
        if tool == "claude":
            env["CLAUDE_PROJECT_DIR"] = str(project)
        env.update(extra)
        return env

    def hivemind(self, project: Path, *args: str) -> subprocess.CompletedProcess:
        return run(
            ["hivemind", "--hivemind-dir", str(project / "hivemind"), "--json", *args],
            cwd=project,
            env=self.env(project),
        )

    def actors_for(self, project: Path, words: str) -> List[str]:
        """The actor ids on the decisions a search for `words` finds."""
        done = self.hivemind(project, "query", "search_decisions", "--q", words, "--limit", "10")
        if done.returncode != 0:
            return []
        try:
            items = json.loads(done.stdout)["data"]["items"]
        except (ValueError, KeyError):
            return []
        return [actor for item in items for actor in item["graph_context"]["actor_ids"]]


def run(
    argv: List[str], cwd: Path, env: Dict[str, str], stdin: str = "", timeout: int = 120
) -> subprocess.CompletedProcess:
    return subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        input=stdin,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def tail(done: subprocess.CompletedProcess) -> str:
    return f"exit {done.returncode}\n{done.stdout[-600:]}\n{done.stderr[-600:]}"


# --------------------------------------------------------------------------------------------
# MCP: start the server exactly as the plugin's config says, over stdio
# --------------------------------------------------------------------------------------------


class McpClient:
    def __init__(self, argv: List[str], cwd: Path, env: Dict[str, str]) -> None:
        self.proc = subprocess.Popen(
            argv,
            cwd=cwd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.next_id = 0
        self.watchdog = threading.Timer(RPC_TIMEOUT_S, self.proc.kill)
        self.watchdog.start()

    def request(self, method: str, params: Dict[str, Any]) -> Dict[str, Any]:
        self.next_id += 1
        assert self.proc.stdin and self.proc.stdout
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("the server closed without answering")
        return json.loads(line)

    def close(self) -> str:
        self.watchdog.cancel()
        if self.proc.stdin:
            self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        return self.proc.stderr.read() if self.proc.stderr else ""


def tool_result(response: Dict[str, Any]) -> Dict[str, Any]:
    result = response.get("result") or {}
    structured = result.get("structuredContent")
    if isinstance(structured, dict):
        return structured
    text = "".join(part.get("text", "") for part in result.get("content", []))
    try:
        parsed = json.loads(text)
    except ValueError:
        return {"error": text or str(response.get("error"))}
    return parsed if isinstance(parsed, dict) else {"error": text}


def check_mcp(world: World, label: str, config: Path, tool: str) -> None:
    server = json.loads(config.read_text())["mcpServers"]["hivemind"]
    project = world.project(f"mcp-{label}")
    argv = [server["command"], *server["args"]]
    env = world.env(project, **server.get("env", {}))
    title = f"Serve the {label} plugin at the release it was built against"
    client = McpClient(argv, project, env)
    stderr = ""
    try:
        init = client.request(
            "initialize",
            {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "check", "version": "0"}},
        )
        connected = check(
            f"{label} MCP server connects", "serverInfo" in (init.get("result") or {}), json.dumps(init)
        )
        if not connected:
            return
        listed = client.request("tools/list", {})
        names = [t["name"] for t in (listed.get("result") or {}).get("tools", [])]
        check(f"{label} MCP server lists capture_decision", "capture_decision" in names, str(names))
        reply = tool_result(
            client.request(
                "tools/call",
                {
                    "name": "capture_decision",
                    "arguments": {
                        "title": title,
                        "rationale": "A plugin that runs ahead of the CLI under it breaks a stranger's install",
                        "topic_keys": ["plugin-compat"],
                        "options": [
                            {"label": "Match the release", "description": "Pass only flags the CLI lists"},
                            {"label": "Match master", "description": "Pass every flag master has"},
                        ],
                        "chosen_option_label": "Match the release",
                        "grounding": [
                            {"kind": "assumption", "statement": "People install the latest release binary"}
                        ],
                    },
                },
            )
        )
        check(
            f"{label} MCP capture_decision is accepted",
            not reply.get("error") and "decision_id" in json.dumps(reply),
            json.dumps(reply)[:600],
        )
    except (RuntimeError, ValueError, OSError) as error:
        check(f"{label} MCP session", False, str(error))
    finally:
        stderr = client.close()
    actors = world.actors_for(project, "release plugin built")
    check(
        f"{label} MCP capture is attributed to agent:{tool}",
        bool(actors) and all(a.startswith(f"agent:{tool}:") for a in actors),
        f"actors={actors} stderr={stderr[-300:]}",
    )


# --------------------------------------------------------------------------------------------
# capture.sh / query-decisions.sh, under each tool's environment
# --------------------------------------------------------------------------------------------

DECISION_FLAGS = [
    "--kind", "decision",
    "--title", "Keep the retry budget at three attempts",
    "--rationale", "More retries hide an outage from the operator instead of surfacing it clearly",
    "--topic-keys", "retries",
    "--options", "Keep 3 attempts,Raise to 5 attempts",
    "--chose", "Keep 3 attempts",
    "--rests-on-assumption", "Operators read the retry dashboard",
]  # fmt: skip


def check_capture_script(world: World, label: str, tool: str) -> Path:
    project = world.project(f"capture-{label}")
    script = str(CAPTURE / "scripts" / "capture.sh")
    env = world.env(project, tool)
    done = run(["bash", script, *DECISION_FLAGS], cwd=project, env=env)
    check(f"capture.sh decision ({label})", done.returncode == 0 and "Captured HiveMind decision" in done.stdout, tail(done))
    done = run(["bash", script, "Retry dashboards were green all week", "--kind", "evidence"], cwd=project, env=env)
    check(f"capture.sh evidence ({label})", done.returncode == 0 and "Captured HiveMind evidence" in done.stdout, tail(done))
    done = run(["bash", script, "Operators check the dashboard daily", "--kind", "hypothesis"], cwd=project, env=env)
    check(f"capture.sh hypothesis ({label})", done.returncode == 0 and "Captured HiveMind hypothesis" in done.stdout, tail(done))
    actors = world.actors_for(project, "retry budget attempts")
    check(
        f"capture.sh decision ({label}) is attributed to agent:{tool}",
        bool(actors) and all(a.startswith(f"agent:{tool}:") for a in actors),
        f"actors={actors}",
    )
    done = run(["bash", str(CAPTURE / "scripts" / "query-decisions.sh"), "retry budget"], cwd=project, env=env)
    check(f"query-decisions.sh finds it ({label})", done.returncode == 0 and "retry budget" in done.stdout.lower(), tail(done))
    return project


# --------------------------------------------------------------------------------------------
# The Codex skill's direct CLI form, used the way the skill says to on an older CLI
# --------------------------------------------------------------------------------------------


def skill_direct_form() -> str:
    skill = (CAPTURE / "skills" / "hivemind-capture" / "SKILL.md").read_text()
    blocks = re.findall(r"```bash\n(.*?)```", skill, flags=re.S)
    form = next(b for b in blocks if "emit decision.capture" in b and "--project-from-context" in b)
    return "\n".join(line[3:] if line.startswith("   ") else line for line in form.splitlines()) + "\n"


def check_skill_direct_form(world: World, label: str, tool: str) -> None:
    project = world.project(f"skill-{label}")
    form = skill_direct_form()
    env = world.env(project, tool, HIVEMIND_DIR=str(project / "hivemind"))
    done = run(["bash", "-c", form], cwd=project, env=env)
    if done.returncode != 0 and "unexpected argument '--project-from-context'" in done.stderr:
        # The skill says: on an older CLI, drop that one flag and re-run.
        stripped = "\n".join(l for l in form.splitlines() if l.strip() != "--project-from-context \\")
        done = run(["bash", "-c", stripped + "\n"], cwd=project, env=env)
    check(f"skill direct form captures ({label})", done.returncode == 0, tail(done))
    actors = world.actors_for(project, "Prefer direct CLI capture before MCP")
    check(
        f"skill direct form ({label}) is attributed to agent:{tool}",
        bool(actors) and all(a.startswith(f"agent:{tool}:") for a in actors),
        f"actors={actors}",
    )


# --------------------------------------------------------------------------------------------
# The AskUserQuestion hooks
# --------------------------------------------------------------------------------------------


def check_ask_hooks(world: World) -> None:
    project = world.project("ask-hooks")
    state = world.root / "hook-state"
    env = world.env(
        project,
        "claude",
        HIVEMIND_DIR=str(project / "hivemind"),
        HIVEMIND_HUMAN_ACTOR="human:alice",
        CLAUDE_PLUGIN_DATA=str(state),
    )
    hook = str(CAPTURE / "scripts" / "ask-hook.sh")
    for phase in ("pre", "post"):
        payload = json.loads((HOOK_FIXTURES / f"{phase}.json").read_text())
        payload["cwd"] = str(project)
        done = run(["sh", hook, phase], cwd=project, env=env, stdin=json.dumps(payload))
        check(
            f"ask hook {phase} stays out of the agent's way",
            done.returncode == 0 and not done.stdout,
            tail(done),
        )
    log = state / "ask-hook" / "hook.log"
    text = log.read_text() if log.exists() else ""
    check(
        "ask hooks never start the CLI with a flag it lacks",
        "unexpected argument" not in text and "exited without answering" not in text,
        text,
    )
    actors = world.actors_for(project, "Postgres hosted")
    check("ask hook post records the human's answer", bool(actors), f"actors={actors}\n{text}")


# --------------------------------------------------------------------------------------------
# The SessionStart directive
# --------------------------------------------------------------------------------------------


def check_session_start_hook(world: World) -> None:
    """The directive names tools the release serves, and the opt-out makes it silent."""
    project = world.project("session-start")
    hook = str(CAPTURE / "scripts" / "session-start-hook.sh")
    done = run(["sh", hook], cwd=project, env=world.env(project, "claude"))
    try:
        context = json.loads(done.stdout)["hookSpecificOutput"]["additionalContext"]
    except (ValueError, KeyError, TypeError):
        context = ""
    check("session start hook adds the directive", done.returncode == 0 and bool(context), tail(done))
    named = [name for name in re.findall(r"`([a-z_]+)`", context) if name != "hivemind"]

    server = json.loads((CAPTURE / ".mcp.json").read_text())["mcpServers"]["hivemind"]
    client = McpClient([server["command"], *server["args"]], project, world.env(project, **server.get("env", {})))
    try:
        client.request(
            "initialize",
            {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "check", "version": "0"}},
        )
        listed = client.request("tools/list", {})
        served = [t["name"] for t in (listed.get("result") or {}).get("tools", [])]
    finally:
        client.close()
    missing = [name for name in named if name not in served]
    check(
        "session start directive names only tools the release serves",
        bool(named) and not missing,
        f"named={named} missing={missing}",
    )

    done = run(["sh", hook], cwd=project, env=world.env(project, "claude", HIVEMIND_DIRECTIVE_DISABLE="1"))
    check(
        "session start hook is silent when disabled",
        done.returncode == 0 and not done.stdout and not done.stderr,
        tail(done),
    )


# --------------------------------------------------------------------------------------------
# hivemind-context
# --------------------------------------------------------------------------------------------


def check_context_scripts(world: World, project: Path, binary_has_ground: bool) -> None:
    scripts = CONTEXT / "scripts"

    def verb(name: str, *args: str, **extra_env: str) -> subprocess.CompletedProcess:
        env = world.env(project, "claude", **extra_env)
        return run(["bash", str(scripts / f"{name}.sh"), *args], cwd=project, env=env)

    for name, args in [
        ("recall", ("retry budget",)),
        ("why", ("retry budget",)),
        ("verify", ("retry budget",)),
        ("situational", ("--paths", "src/retry.rs")),
    ]:
        done = verb(name, *args)
        check(f"hivemind-context {name}", done.returncode == 0, tail(done))
    # An actor cannot accept and reject the same decision, so someone else disagrees.
    done = verb(
        "disagree", "retry budget", "--reason", "Three attempts is too few for a flaky upstream",
        GC_AGENT="check-reviewer",
    )  # fmt: skip
    check("hivemind-context disagree", done.returncode == 0, tail(done))
    done = verb(
        "supersede", "retry budget",
        "--title", "Raise the retry budget to five attempts",
        "--rationale", "The flaky upstream needs more attempts than three to succeed reliably",
        "--topic-keys", "retries", "--options", "Keep 3 attempts,Raise to 5 attempts",
        "--chose", "Raise to 5 attempts", "--rests-on-assumption", "The upstream stays flaky",
    )  # fmt: skip
    check("hivemind-context supersede", done.returncode == 0, tail(done))
    done = verb("ground", "retry budget", "--rests-on-assumption", "The upstream stays flaky")
    if binary_has_ground:
        check("hivemind-context ground", done.returncode == 0, tail(done))
    else:
        check(
            "hivemind-context ground names the verb the release lacks",
            done.returncode == 2 and "has no `ground` command" in done.stderr,
            tail(done),
        )


# --------------------------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    parser.add_argument("--bin", type=Path, help="the hivemind binary to treat as the latest release")
    parser.add_argument("--repo", help="owner/name to download the latest release from (default: gh's)")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="hivemind-plugin-release-") as tmp:
        root = Path(tmp)
        (root / "release").mkdir()
        (root / "world").mkdir()
        binary = args.bin or download_latest_release(args.repo, root / "release")
        version = subprocess.run([str(binary), "--version"], capture_output=True, text=True).stdout.strip()
        print(f"release binary: {version}")
        world = World(root / "world", binary)
        has_ground = subprocess.run([str(binary), "ground", "--help"], capture_output=True).returncode == 0

        check_mcp(world, "Claude Code", CAPTURE / ".mcp.json", "claude")
        check_mcp(world, "Codex", CAPTURE / ".codex-plugin" / "mcp.json", "codex")
        claude_project = check_capture_script(world, "Claude Code", "claude")
        check_capture_script(world, "Codex", "codex")
        check_skill_direct_form(world, "Claude Code", "claude")
        check_skill_direct_form(world, "Codex", "codex")
        check_ask_hooks(world)
        check_session_start_hook(world)
        check_context_scripts(world, claude_project, has_ground)

    print()
    if FAILURES:
        print(f"{len(FAILURES)} check(s) failed against {version}:")
        for name in FAILURES:
            print(f"  - {name}")
        return 1
    print(f"all checks passed against {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
