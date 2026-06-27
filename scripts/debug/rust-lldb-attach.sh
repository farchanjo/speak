#!/usr/bin/env bash
# rust-lldb-attach.sh — attach to a LIVE process, dump all-thread state, DETACH
# (never kills it). Built for the speak daemon: "why is it hung / what is each
# thread doing right now".
#
# Usage:  scripts/debug/rust-lldb-attach.sh [PID]
#   PID omitted -> reads ~/.speak/speak.pid (the running daemon).
#
# Prints every thread's backtrace, then detaches cleanly so the daemon keeps
# running. Read-only inspection — safe on a live process.
set -euo pipefail
PID="${1:-}"
PIDFILE="${SPEAK_PIDFILE:-$HOME/.speak/speak.pid}"
if [[ -z "$PID" ]]; then
  [[ -f "$PIDFILE" ]] || { echo "rust-lldb-attach: no PID given and no $PIDFILE" >&2; exit 1; }
  PID="$(tr -dc '0-9' < "$PIDFILE")"
fi
kill -0 "$PID" 2>/dev/null || { echo "rust-lldb-attach: pid $PID not alive" >&2; exit 1; }

TIMEOUT="${TIMEOUT:-30}"
_to() { if command -v timeout >/dev/null 2>&1; then timeout -s KILL "$1" "${@:2}"; else gtimeout -s KILL "$1" "${@:2}"; fi; }

echo "attaching to pid $PID (read-only, will detach)…" >&2
_to "$TIMEOUT" rust-lldb -b \
  -o "settings set interpreter.echo-commands false" \
  -o "process attach --pid $PID" \
  -o "thread backtrace all" \
  -o "detach" \
  -o "quit" 2>&1 \
  | awk '/Process [0-9]+ stopped|thread #/{p=1} p' \
  | grep -vE "type (synthetic|summary) add|category Rust|lldb_lookup\.py"
