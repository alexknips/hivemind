#!/bin/sh
# Claude Code hook entry for AskUserQuestion (hivemind-bbnw.5): `ask-hook.sh pre` records the
# question as an ask, `ask-hook.sh post` records the human's answer as a decision linked to it.
# See ask_hook.py. A machine without python3 loses the recording, never the question.
command -v python3 >/dev/null 2>&1 || exit 0
exec python3 "$(dirname "$0")/ask_hook.py" "$@"
