---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Daemon single-instance lock, health watchdog, and forwarded-translate routing

## Context and Problem Statement

The persistent daemon (ADR-0005) is a long-lived process that holds one warm
client pool behind a Unix socket. Three gaps remained once it was in regular use:

1. **No single-instance guarantee.** A second `speak daemon` would `bail!`
   ("already running") instead of taking over, and a crash could leave a stale
   socket that blocks the next start. There was no record of which process owns
   the socket, so nothing could reliably stop or replace a previous instance.
2. **No upstream supervision.** If the upstream API became unreachable or wedged,
   the daemon kept serving stale/failing requests with no detection, no recovery,
   and no operator-visible health. The user asked specifically to "check the
   timeout between the API endpoint and be able to restart and do everything".
3. **Forwarded translation ignored `--to`.** The daemon's warm Facade held a bare
   `Retry<OpenAiAdapter>` whose `Translator` only emits English via Whisper
   translate. A forwarded `translate`/`realtime` request to a non-English target
   was silently answered in English, unlike the in-process path which routes
   non-English targets through the chat-MT Strategy (ADR-0004).

The daemon is only ever started by `speak` itself (the `daemon` subcommand); it is
never launched by an external supervisor. So the single-instance, supervision, and
recovery logic must live inside the binary.

## Decision Drivers

- Running `speak daemon` again must REPLACE the previous instance, deterministically.
- A clean exit must never leave a stale lock or socket; a crash must be recoverable.
- Detect upstream timeouts/failures and self-recover (rebuild the pool, re-probe
  capability) without an operator restart, while staying crash-free when degraded.
- Keep the supervision logic a small, deterministically testable state machine.
- A forwarded request must take the identical use-case path as in-process — same
  translation Strategy selection — so the daemon is transparent (ADR-0005).
- No `unsafe`; no external process exec (Constitution Principle 5 / the gate).
- Every new tunable is an env-overridable `SPEAK_*` knob with a default (FR-18).

## Considered Options

- **Single-instance lock**: (A) a PID file with liveness + socket-ping verification
  and SIGTERM→SIGKILL replace; (B) `flock` on the socket's directory; (C) refuse to
  start when a socket exists (the prior behaviour).
- **Supervision**: (A) an in-process watchdog task probing `/health` on a cadence
  with a timeout and a failure-count threshold that triggers self-recovery; (B) an
  external supervisor (systemd/launchd) — rejected because the daemon is only ever
  spawned by `speak` itself; (C) no supervision (the prior behaviour).
- **Forwarded translate**: (A) hold the same in-process composite (retry-wrapped
  `openai` + chat-MT, target-routed) in the daemon Facade; (B) thread a translate
  Strategy selector across the wire protocol — rejected as redundant since the
  target already crosses the wire and the composite already encodes the routing.

## Decision Outcome

Chosen option: "single-instance PID file (A) + in-process health watchdog (A) +
shared in-process speech composite (A)" — the PID-file lock with SIGTERM→SIGKILL
replace, the in-process watchdog with self-recovery, and the daemon Facade holding
the same target-routed speech composite as the CLI.

### Single-instance lifecycle (`adapters/daemon/lifecycle.rs`)

- A PID file at `[daemon].pidfile` (`SPEAK_DAEMON_PIDFILE`, default
  `~/.speak/speak.pid`). On start the daemon reconciles any previous instance:
  - **PID alive AND answering on the socket** ⇒ it is our daemon: SIGTERM, wait up
    to `[daemon].kill_grace_ms` (`SPEAK_DAEMON_KILL_GRACE_MS`, default 3000), then
    SIGKILL if it lingers; remove its socket + pidfile.
  - **PID alive but silent on the socket** ⇒ likely PID reuse, not our daemon:
    leave the process untouched and treat the pidfile as stale.
  - **PID dead / pidfile absent** ⇒ clean the stale pidfile + leftover socket. An
    orphan daemon with no pidfile is stopped over its own socket.
- After binding the socket, the daemon writes its own PID **atomically** (temp file
  + rename). On graceful shutdown (SIGINT or SIGTERM) it removes BOTH the pidfile
  and the socket, so a clean exit never leaves a stale lock.
- `daemon stop` SIGTERMs the pidfile PID (waiting out the grace) then cleans up;
  `daemon restart` is a fresh start (which already replaces the previous instance);
  `daemon status` reports running/pid/uptime/socket/pidfile + the watchdog snapshot
  through the Presenter port (`--json` renders the same structure, ADR-0009).
- Process signalling uses the `nix` safe wrappers (no `unsafe`) and is gated to
  `cfg(unix)`; the pidfile read/write is portable. `nix` is added only here.

### Health watchdog + self-recovery (`adapters/daemon/watchdog.rs`)

- A background task probes the upstream `/health` through the warm Facade's
  `ServerProbe` port — the exact capability surface the CLI uses, so it is fully
  mockable — every `[daemon].health_interval` seconds (`SPEAK_DAEMON_HEALTH_INTERVAL`,
  default 15; 0 disables) with a per-probe `SPEAK_HEALTH_TIMEOUT` (default 5s) bound.
- The transition logic is a pure state machine (`Health`): `Healthy → Degraded →
  Recovering → Healthy`. After `[daemon].health_fails` (`SPEAK_DAEMON_HEALTH_FAILS`,
  default 3) consecutive failures it triggers **self-recovery**: rebuild the warm
  `openai`/`reqwest` client pool and re-run the realtime capability probe (so the
  SSE `/v1/realtime/translate` endpoint is rediscovered when the server returns),
  then hot-swap the fresh Facade in. While degraded the loop backs off through the
  shared `[retry]` policy. A restored probe transitions back to `Healthy`.
- The warm Facade is held behind a `Mutex<Arc<…>>` so recovery can hot-swap a fresh
  pool while connection handlers clone the inner `Arc` under a momentary guard —
  no lock is ever held across `.await`. `daemon status` surfaces the state, the
  consecutive-failure count, seconds-since-last-OK, the last error, and the
  recovery count.
- The state machine is unit-tested with scripted probe outcomes and injected
  timestamps (no real sleeps): degrade on the first failure, recover-trigger on the
  Nth, stay `Recovering` without re-triggering, and return to `Healthy` on success.

### Forwarded-translate routing (`adapters/inproc.rs`)

- The in-process warm speech stack is extracted into one reusable composite,
  `InProcessSpeech` (retry-wrapped `openai` + optional chat-MT, `translate` routed
  by target language). Both in-process callers share it: the CLI's
  `SpeechRole::Direct` and the daemon's warm Facade. A forwarded non-English
  `translate`/`realtime` request therefore honours `--to` exactly like in-process —
  the target already crosses the wire (`Request::Translate { target }`), and the
  composite encodes the Strategy selection (ADR-0004), so no new wire field is
  needed and the domain stays serde-free (ADR-0003).

### Consequences

- Good: deterministic single-instance ownership; no stale locks on clean exit;
  self-healing across upstream blips with operator-visible health; transparent
  forwarded translation; zero `unsafe`; no external process exec.
- Good: the supervision logic is a pure, fast state machine, decoupled from the
  async probe loop and the clock.
- Bad: one new `cfg(unix)` dependency (`nix`); the daemon is macOS/Linux only (as
  before — no Windows named-pipe path); recovery rebuilds the whole Facade rather
  than reusing connections (cheap: construction touches no network).

### Caveat

Subtitle output for `speak translate --format srt|vtt` (also landed alongside this
work) is built from the server's transcription SEGMENTS via `/v1/audio/transcriptions`
(which emits timestamped SRT/VTT) routed through the `Transcriber` port, so the cues
are in the SOURCE language; `--to` applies only to the text formats. Translated
subtitles (English via `/v1/audio/translations`, which also supports `srt`/`vtt`)
are a noted enhancement that would thread the response format through the
`Translator` Strategy.
