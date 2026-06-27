---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Local hardware acceleration probe and rotating logs

## Context and Problem Statement

The operator wants `speak` to (1) detect the host OS, probe the locally
available hardware acceleration, and use the best one, with an env override of
the auto-detect; and (2) write an intelligent, rotating log under the project
home `~/.speak/logs`, also controlled by env. The catch: `speak` processes
**audio only**, and GPU video codecs (NVENC/NVDEC, VideoToolbox) do not
accelerate audio decoding.

## Decision Drivers

- Be honest: do not claim a GPU audio path that does not exist.
- Still "use everything local" that genuinely helps audio.
- Everything overridable via environment variables.
- Logs must not grow unbounded and must live in `~/.speak/logs`.

## Considered Options

- Option A — Probe + report local acceleration, apply CPU frame-threading and
  Apple AudioToolbox audio decoders; `tracing` + `tracing-appender` rotating
  file logs.
- Option B — Attempt GPU (CUDA/VideoToolbox) hardware decode for audio.
- Option C — No probe; rely on libav defaults; log to stderr only.

## Decision Outcome

Chosen option: "Option A".

- Acceleration (`src/accel.rs`): `speak check` reports OS/arch, CPU cores,
  libavcodec version, libav hwdevice types, and the AudioToolbox `*_at`
  decoders actually present, plus the effective policy. Decoding uses libav
  **frame threading across all cores** and, on macOS under `auto`, the
  AudioToolbox decoder for the stream's codec (e.g. `mp3_at`, `aac_at`) when
  available, falling back to the software decoder on any failure. The policy is
  overridable with `SPEAK_HWACCEL=auto|off|<decoder-name>`. GPU acceleration is
  deliberately not used for audio (NVENC/NVDEC/VideoToolbox are video codecs;
  the only GPU is the server's, running TTS/ASR inference).
- Logging (`src/logging.rs`): `tracing` with a `tracing-appender` daily-rotating
  file appender under `~/.speak/logs` (override `SPEAK_LOG_DIR`), retention
  capped at 7 files, non-blocking writer. Level/filter via `SPEAK_LOG`
  (`SPEAK_LOG=off` disables file logging). User-facing results stay on
  stdout/stderr; diagnostics (including reqwest connection pooling) go to the
  log.

### Consequences

- Good: honest, OS-aware acceleration that actually helps (threading +
  AudioToolbox); a discoverable `speak check`; bounded, env-driven logs.
- Good: rejecting option B avoids a misleading and unimplementable GPU
  audio-decode path.
- Enforced: the libav decode path is strictly **in-process** — a `tests/gates.rs`
  scan (`zero_media_exec_in_src`) fails the build on any external-process spawn
  in `src/`, so `speak` can never regress to shelling out `ffmpeg`/`afplay`.
- Bad: the AudioToolbox speed-up for tiny TTS/ASR clips is marginal (decode is
  already a fraction of inference time); the main win is correctness and
  observability rather than raw throughput.
