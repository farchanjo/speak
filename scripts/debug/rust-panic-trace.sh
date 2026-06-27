#!/usr/bin/env bash
# rust-panic-trace.sh — run a command under lldb and, if it panics, dump the
# backtrace + locals at the panic site (before unwind eats the frames).
#
# Usage:  scripts/debug/rust-panic-trace.sh [-b BIN] [-t SECS] -- <args...>
# Example: scripts/debug/rust-panic-trace.sh -- say "hi"
#
# Breaks on `rust_panic` (the panic entry, panic=unwind) so you see exactly which
# frame and which values triggered it — no more guessing from the panic string.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# Panic breakpoints/commands FIRST so they land before any `--` in "$@".
exec "$HERE/rust-lldb-batch.sh" \
  -k '--name rust_panic' \
  -k '--name core::panicking::panic_fmt' \
  -c 'thread backtrace -c 24' \
  -c 'frame variable' \
  -c 'continue' \
  "$@"
