#!/usr/bin/env bash
# rust-lldb-batch.sh — drive lldb HEADLESS to ground analysis on real runtime state.
#
# Purpose: instead of guessing a struct's fields, a value, or a call path from
# source, stop the live process and read the truth. lldb corrects wrong field
# names and prints actual values — this is the anti-hallucination loop.
#
# Usage:
#   scripts/debug/rust-lldb-batch.sh [-b BIN] [-t SECS] [-k 'BP']... [-c 'CMD']... -- [PROG ARGS...]
#
#   -b BIN    target binary           (default: target/debug/speak)
#   -t SECS   hard timeout            (default: 60 — lldb Rust synthetic
#                                       formatters can hang on whole-struct dumps)
#   -k 'BP'   breakpoint spec, repeatable. Passed verbatim to `breakpoint set`,
#             e.g. -k '--file main.rs --line 111'  |  -k '--name rust_panic'
#   -c 'CMD'  lldb command at the stop, repeatable (default: `thread backtrace -c 12`)
#   --        everything after is the program + its args (used by `run`)
#
# Examples:
#   # backtrace + a value at dispatch:
#   scripts/debug/rust-lldb-batch.sh -k '--file main.rs --line 111' \
#       -c 'thread backtrace -c 8' -c 'p cfg->server.host' -c 'p cfg->server.timeout' \
#       -- config path
#   # all-thread state of a hung run:
#   scripts/debug/rust-lldb-batch.sh -k '--name rust_panic' -c 'thread backtrace all' -- say hi
#
# Rule of thumb: read SPECIFIC members (`p cfg->server.host`), never `frame
# variable *cfg` — the whole-struct synthetic walk can stall and trip the timeout.
set -euo pipefail

BIN="target/debug/speak"
TIMEOUT=60
BPS=()
CMDS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -b) BIN="$2"; shift 2 ;;
    -t) TIMEOUT="$2"; shift 2 ;;
    -k) BPS+=("$2"); shift 2 ;;
    -c) CMDS+=("$2"); shift 2 ;;
    --) shift; break ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "rust-lldb-batch: unknown arg '$1'" >&2; exit 2 ;;
  esac
done
PROG_ARGS=("$@")

[[ -x "$BIN" ]] || { echo "rust-lldb-batch: binary not found: $BIN (run 'cargo build')" >&2; exit 1; }
[[ ${#CMDS[@]} -eq 0 ]] && CMDS=("thread backtrace -c 12")

# Portable hard timeout: prefer GNU timeout/gtimeout, else a bash watchdog.
_run_timeout() { # _run_timeout SECS cmd...
  local secs="$1"; shift
  if command -v timeout >/dev/null 2>&1; then timeout -s KILL "$secs" "$@"
  elif command -v gtimeout >/dev/null 2>&1; then gtimeout -s KILL "$secs" "$@"
  else
    "$@" & local pid=$!
    ( sleep "$secs"; kill -9 "$pid" 2>/dev/null ) & local w=$!
    local rc=0; wait "$pid" 2>/dev/null || rc=$?
    kill -9 "$w" 2>/dev/null || true
    return "$rc"
  fi
}

# Assemble the lldb argv (echo off keeps the formatter-import banner quiet too).
LLDB=(rust-lldb -b -o "settings set interpreter.echo-commands false")
for bp in "${BPS[@]}"; do LLDB+=(-o "breakpoint set $bp"); done
LLDB+=(-o "run")
for c in "${CMDS[@]}"; do LLDB+=(-o "$c"); done
LLDB+=(-o "quit" -- "$BIN")
[[ ${#PROG_ARGS[@]} -gt 0 ]] && LLDB+=("${PROG_ARGS[@]}")

# Run, then strip the Rust pretty-printer registration banner — keep only the
# session from the first process launch/stop onward.
RUST_BACKTRACE=1 _run_timeout "$TIMEOUT" "${LLDB[@]}" 2>&1 \
  | awk '/Process [0-9]+ (launched|stopped|exited)|stop reason/{p=1} p' \
  | grep -vE "type (synthetic|summary) add|category Rust|lldb_lookup\.py|Executing commands in"
