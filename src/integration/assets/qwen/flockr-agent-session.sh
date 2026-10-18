#!/bin/sh
# managed by flockr; reinstalling the integration replaces this file.
# FLOCKR_INTEGRATION_ID=qwen
# FLOCKR_INTEGRATION_VERSION=2

[ "${1:-}" = "session" ] || exit 0
[ "${FLOCKR_ENV:-}" = "1" ] || exit 0
[ -n "${FLOCKR_PANE_ID:-}" ] || exit 0
[ -n "${FLOCKR_SOCKET_PATH:-}" ] || exit 0
if [ -n "${FLOCKR_BIN_PATH:-}" ]; then
    [ -x "$FLOCKR_BIN_PATH" ] || exit 0
else
    command -v flockr >/dev/null 2>&1 || exit 0
fi
command -v python3 >/dev/null 2>&1 || exit 0

python3 -c '
import json
import os
import subprocess
import sys
import time

try:
    payload = json.load(sys.stdin)
    session_id = payload.get("session_id")
    source = payload.get("source")
    if not isinstance(session_id, str) or not session_id:
        raise ValueError
    command = os.environ.get("FLOCKR_BIN_PATH") or "flockr"
    args = [
        command, "pane", "report-agent-session", os.environ["FLOCKR_PANE_ID"],
        "--source", "flockr:qwen", "--agent", "qwen",
        "--agent-session-id", session_id, "--seq", str(time.time_ns()),
    ]
    if source in ("startup", "resume", "clear", "compact", "branch"):
        args.extend(["--session-start-source", source])
    subprocess.run(
        args,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=1,
        check=False,
    )
except Exception:
    pass
' 2>/dev/null || true
