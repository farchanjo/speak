---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# TCC responsibility-disclaim re-exec for host-output capture

## Context and Problem Statement

The native Core Audio output tap (ADR-0015) needs the macOS audio-capture grant
`kTCCServiceAudioCapture`. macOS attributes a TCC request to the **responsible
process**, which for a CLI is the launching terminal — not the binary. Verified
on-device: a signed `speak` run directly from a shell taps **silence** (the tap
runs but is muted) because the terminal (iTerm/Terminal) holds no audio-capture
grant; only when launched via LaunchServices (`open speak.app`) — which makes the
app its own responsible process — does the grant on `ltd.eonf.speak` apply and
the tap deliver audio (confirmed at −24 dBFS).

`open`-launching is unusable for a streaming CLI (`transcribe --stream` needs the
terminal's stdout + Ctrl-C). We want `speak transcribe --stream --source output`
to work seamlessly from any terminal, without granting that terminal
audio-capture and without `open`.

## Decision Drivers

- Make `speak` its **own** TCC subject so the grant on its code identity applies
  regardless of the launching terminal.
- Keep normal stdout/stdin/Ctrl-C behavior for the streaming pipeline.
- Confine the mechanism to the commands that actually capture host output.
- Stay inside the zero-media-exec gate (no external-process media exec).

## Considered Options

- **Option A** — Document `open speak.app` / "grant your terminal audio-capture".
  No code, but `open` breaks streaming and per-terminal grants are fragile.
- **Option B** — A login-item / LaunchAgent that owns the tap. Heavy; wrong shape
  for a CLI.
- **Option C** — **Self-re-exec with TCC disclaim.** On the capture commands,
  `speak` re-execs itself once via `posix_spawn` with the private
  `responsibility_spawnattrs_setdisclaim` attribute (the pattern terminal
  emulators use): the child disclaims the parent's TCC responsibility and becomes
  its own responsible process, so its code-signing identity (`ltd.eonf.speak`) is
  the TCC subject and the persisted grant applies. The parent supervises.

## Decision Outcome

Chosen option: **Option C**.

- The composition root (`main.rs::pre_dispatch_disclaim`) runs **before** logging
  and the async runtime. Only the three capture commands
  (`transcribe`/`record`/`realtime`) can target `--source output`, so it loads
  config and calls `cli::wants_output_capture` for them only; everything else is
  untouched.
- When the resolved source is `output`, `coreaudio::reexec_disclaimed`
  (`adapters/coreaudio/macos/disclaim.rs`, macOS only; a no-op stub elsewhere)
  `posix_spawn`s `current_exe` with: `responsibility_spawnattrs_setdisclaim(attr,
  1)`, `POSIX_SPAWN_SETSIGDEF` for `SIGINT/SIGTERM/SIGHUP/SIGQUIT` (so Ctrl-C
  still stops the child while the supervisor ignores it), the original argv, and
  the inherited environment plus a `SPEAK_TCC_DISCLAIMED=1` sentinel. The
  sentinel makes the disclaimed child return immediately and run the work; the
  parent ignores terminal signals, `waitpid`s, and exits with the child's status.
- This is the **second** sanctioned `current_exe` self-exec, alongside the daemon
  autostart (ADR-0005). It uses `posix_spawn` (not `std::process::Command`), so it
  is outside the zero-media-exec gate's `Command::new`/`process::Command`/
  `tokio::process` deny-list, and its `current_exe` line is on the gate's
  allow-path. It never execs an external binary — only `speak` itself.
- Effective only for a **code-signed** binary (the `make app` bundle). An ad-hoc
  binary disclaims to an identity with no grant and still falls back to the
  all-zero-capture `tracing` warning that names `make app`.

Verified on-device: after `make app` + a one-time grant, **direct-exec** of the
bundle binary captured a 440 Hz tone at mean −27.6 dBFS / peak −8.5 dBFS — no
`open`, no terminal grant.

### Consequences

- Good: `speak transcribe --stream --source output` (and `record`/`realtime`)
  work seamlessly from any terminal once the bundle's identity is granted; the
  grant persists by team id across rebuilds.
- Good: scoped to output-capture commands; all other commands keep a single
  process and are unaffected.
- Good: `posix_spawn` is async-signal-safe and thread-safe (unlike `fork`), so it
  is sound even though the check runs inside the `#[tokio::main]` entry.
- Bad: output capture spawns a short-lived supervisor parent (one extra process)
  for the capture commands; the parent forwards the exit code and signals.
- Bad: relies on the private `responsibility_spawnattrs_setdisclaim` SPI (stable
  and widely used, but undocumented) — isolated behind one macOS-only function.
- Neutral: extends ADR-0005's single-self-exec allowance to a second, equally
  bounded `current_exe` site; the zero-media-exec gate still forbids every
  external-process media exec.
