#!/bin/sh
# Claude Code SessionStart hook (hivemind-wdwg): tell the agent, as additional context, to check
# the HiveMind decision ledger before it acts and to record the decisions it settles on.
#
# Without it, an installed plugin with a connected MCP server went unused: 0 captures and 0
# recalls in 31 headless benchmark sessions, because the plugin's only hooks fire on
# AskUserQuestion, which an autonomous agent never calls. The text names the tools to call; it
# decides nothing and reads no ledger (the three layers stay apart: this is a fixed sentence).
#
# It fails open. With HIVEMIND_DIRECTIVE_DISABLE set, or no `hivemind` binary to run the plugin's
# MCP server, it prints nothing and exits 0. A ledger directory that does not exist yet is a new
# ledger, not a missing one: the MCP server creates it on the first capture, and the first
# session in a project is exactly where the directive is needed. Plain sh, no python3.
#
# Environment:
#   HIVEMIND_DIRECTIVE_DISABLE  any value: print nothing.
#   HIVEMIND_CAPTURE_BIN        the hivemind binary; default is `hivemind` on PATH.
[ -n "${HIVEMIND_DIRECTIVE_DISABLE:-}" ] && exit 0
command -v "${HIVEMIND_CAPTURE_BIN:-hivemind}" >/dev/null 2>&1 || exit 0

# Keep it short: it is paid for in every session. The tests hold it under 400 tokens.
cat <<'JSON'
{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"HiveMind decision ledger: before you act on a task, call the `hivemind` MCP tool `recall_decisions` (q = three or four key words about the task) for decisions made in earlier sessions, and follow the ones that still hold. Each time you settle a durable decision or rule, call `capture_decision` with its title, rationale (the why), options considered, and what it rests on (grounding), so later sessions can follow it."}}
JSON
