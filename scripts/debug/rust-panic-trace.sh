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
#
# Why --func-regex and not --name: the panic lowering differs by edition + how std
# is linked. `panic!`-with-fmt -> core::panicking::panic_fmt; `assert!`/bare panic
# -> core::panicking::panic OR (older editions) std::panicking::begin_panic;
# assert_eq! -> core::panicking::assert_failed; the std hook `rust_panic` is v0-
# mangled so `--name rust_panic` stays *pending*. A regex over the panic families
# catches every monomorphization regardless — proven: it stops at the panic site
# with the real args (e.g. `divide(numerator=42, denominator=0)`), where the
# name-only list silently missed (hit count 0). A pending entry just warns.
exec "$HERE/rust-lldb-batch.sh" \
  -k '--func-regex core::panicking::(panic|assert_failed)' \
  -k '--func-regex std::panicking::(begin_panic|rust_panic)' \
  -c 'thread backtrace -c 24' \
  -c 'frame variable' \
  -c 'continue' \
  "$@"
