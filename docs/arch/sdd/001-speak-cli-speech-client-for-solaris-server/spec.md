# Feature Specification: Speak Cli Speech Client For Solaris Server

Feature: 001-speak-cli-speech-client-for-solaris-server
Created: 2026-06-26
Status: accepted

## Implementation Status

The speckit phase marker is `implement` / status `implemented`: the feature is
behaviorally implemented as a working client (`say`/`tts`, `transcribe`,
`translate`, `realtime`, `voices`, `daemon`, `check`, `health`, `config`,
`completions`) in the original **flat** module layout (`client.rs`, `codec.rs`,
`audio_macos.rs`, `daemon.rs`, `config.rs`, ...). The Hexagonal + DDD + GoF
layout of ADR-0003 (the `domain`/`ports`/`application`/`adapters`/`cli` tree),
the `async-openai` + `eventsource-stream` migration, the `[retry]`/`[http]`
config sections, the `record`/`devices` commands, the repeatable
`--output-device` fan-out, the `--translate`/`--no-translate`/`--echo` realtime
modes, the `Presenter` output port + `tracing` diagnostics discipline (ADR-0009,
replacing the scattered `println!`/`eprintln!`), and the edition-2024 bump are an
**accepted but in-progress refactor**.
Their tasks remain unchecked in `tasks.md` (whose checkbox legend reads
`[x]` = present in the current tree, `[ ]` = pending for the hexagonal rebuild),
so the `implemented` marker denotes the shipped flat-layout behavior, NOT
completion of the layered architecture. This section is the single place that
reconciles the speckit status with the source tree to avoid a silent mismatch.

As of 2026-06-27 the layered tree is in place and an architecture-discipline
cleanup (T064) closed the last two gaps: the four flat-root framework modules
(`client.rs`, `accel.rs`, `config.rs`, `daemon.rs`) were relocated under
`src/adapters/` so framework crates appear only under `adapters/` (and clap
under `cli/`), and `src/domain` was purified of `serde_json`/`anyhow` (the
`GenParams` value object replaces the raw JSON map; the validators return
`DomainError`). The only flat-root `src/*.rs` files left are `main.rs`
(composition root), `lib.rs`, and the cross-cutting `logging.rs`/`paths.rs`.

## Summary

`speak` is a single self-contained Rust binary: a network client for the
OpenAI-compatible speech server (v2.3) at `http://solaris:8800` (OmniVoice TTS +
faster-whisper ASR on an RTX 4090). It provides Text-to-Speech (with voice
design, voice cloning, saved-voice management, and generation-parameter tuning),
Speech-to-Text, audio translation, a realtime microphone pipeline (SSE-streamed
when the server supports it, chunked otherwise), microphone recording, audio
device discovery, and an optional persistent daemon for warm connections. It is
trivially configurable through a layered config catalog and works fully over the
network. All media is handled in-process (libav for codecs, native macOS
CoreAudio for device I/O) with zero process exec.

The codebase follows Hexagonal architecture with DDD and named GoF patterns
(ADR-0003): a pure `domain`, `ports` traits, `application` use cases, and
`adapters` for OpenAI HTTP, CoreAudio, libav, config, daemon, and SSE. Command
results are emitted through a swappable `Presenter` output port and diagnostics
flow through `tracing` (ADR-0009), so no raw `println!` leaks into the layers.

## User Stories

- As a CLI user I want `speak say "texto"` to synthesize speech on the server and
  play it locally, so that I get high-quality TTS from one short command.
- As a CLI user I want to design a voice from canonical tags
  (`--instruct "Female, Young Adult, British Accent"`) or clone a saved voice
  (`--voice <name> --ref-text ...`), so that I control how the output sounds.
- As a CLI user I want `speak voices add|list|rm` to register, list, and delete
  cloneable voices on the server, so that I can reuse them by name.
- As a CLI user I want `speak transcribe audio.mp3` and `speak translate audio.mp3`
  to return a transcript or an English translation, so that I can turn audio into
  text and understand foreign-language audio.
- As a CLI user I want `speak realtime` to capture my microphone and either
  translate, re-voice (no-translate), or echo it live to one or many output
  devices, so that I get hands-free live translation or voice changing.
- As a CLI user I want `speak record` to capture the microphone to a WAV/FLAC
  file, and `speak devices` to list input/output devices, so that I can manage
  local audio.
- As a power user I want `speak daemon` to hold a warm pooled connection so that
  repeated commands ride an already established socket.
- As a user I want configuration via a TOML file, environment variables, and
  flags with clear precedence, and `config show` to tell me where each value came
  from, so that I can set defaults once and override per call and always know why
  a value is in effect.
- As an integrator I want the client to speak both the OpenAI audio API and the
  server's native `/tts`, and to target any compatible server via `--host`, so
  that I can use whichever endpoint and backend I prefer.

## Functional Requirements

1. **TTS** — `speak say|tts <text>` POSTs to `/v1/audio/speech` (OpenAI schema
   `model,input,voice,response_format,speed,language`) or native `/tts` when
   `--native`. Default `language=pt-BR`, `format=mp3`. Plays the decoded audio
   locally unless `-o FILE` or `--no-play`. `--format` selects
   `mp3|opus|aac|flac|wav|pcm`; `--duration` and `--speed` tune output.
   `--output-device` is repeatable (single device or fan-out, FR-11). When `-o`
   is a bare filename (no directory component) the file lands under
   `[http].save_dir` (`SPEAK_SAVE_DIR`); an absolute or directory-qualified `-o`
   path is honoured as given, and an unset `save_dir` resolves to the current
   working directory. On `--json` (FR-16) the synthesis result also surfaces the
   server's inference-timing response headers `X-RTF` and `X-Audio-Seconds` when
   the server returns them.
2. **Voice modes** — exactly one of: `--voice <saved-name>` (clone, optionally
   with `--ref-text`); `--instruct "<tags>"` (voice design, FR-3); or neither
   (server default/auto). `--list-designs` prints the canonical tag vocabulary.
3. **Voice design vocabulary** — `--instruct` is a comma-separated list of
   CANONICAL tags only (free text errors server-side; the domain validates
   before sending). The 23 English tags are: `male, female, child, teenager,
   young adult, middle-aged, elderly, very low pitch, low pitch, moderate pitch,
   high pitch, very high pitch, whisper, american accent, australian accent,
   british accent, canadian accent, chinese accent, indian accent, japanese
   accent, korean accent, portuguese accent, russian accent`.
4. **Generation parameters** — repeatable `--set key=value` passes validated
   gen-params through to the server: `num_step` (alias `steps`), `guidance_scale`,
   `t_shift`, `layer_penalty_factor`, `position_temperature`, `class_temperature`,
   `denoise`, `preprocess_prompt`, `postprocess_output`, `audio_chunk_duration`,
   `audio_chunk_threshold`. These map to `[tts.gen]` config defaults.
5. **Voice management** — `speak voices add|list|rm` wraps the server's
   `POST /voices` (multipart `name,audio,ref_text?`), `GET /voices`, and
   `DELETE /voices/{name}` behind the `VoiceRepository` port.
6. **STT** — `speak transcribe <file>` POSTs multipart to
   `/v1/audio/transcriptions` (`model=whisper-1`, optional `--language`,
   `--format json|text|srt|vtt|verbose_json`).
7. **Translate (file)** — `speak translate <file>` POSTs multipart to
   `/v1/audio/translations` (audio to English text). `--format srt|vtt` instead
   emits timestamped subtitle cues built from the server's transcription
   SEGMENTS (the `/v1/audio/transcriptions` endpoint emits SRT/VTT), routed
   through the `Transcriber` port — honouring the `--format` arg for those formats
   (ADR-0010). Subtitle cues are SOURCE-language; `--to` applies to the text
   formats only.
8. **Realtime pipeline** — `speak realtime` captures the microphone in chunks
   (native CoreAudio tap, `--chunk` seconds, silence-split) and runs one of three
   modes, then plays the result through chosen `--output-device`(s):
   - `--translate` (default per `[realtime].translate`): ASR -> MT. English
     target uses Whisper translate; an arbitrary `--to` target requires a chat
     MT endpoint (`translate_url`/`translate_model`), else it degrades to the
     source transcript with a clear notice.
   - `--no-translate`: passthrough re-voice (ASR -> TTS in the chosen output
     voice).
   - `--echo`: raw captured audio is played back, then re-voiced via TTS.
   Output voice is `--instruct` (design) or `--voice` (clone) or default.
   `--from`/`--to` set languages. When the server's SSE endpoint
   `POST /v1/realtime/translate` is available it is consumed frame-by-frame;
   otherwise the client falls back to the chunked ASR->MT->TTS loop. The SSE
   path is selected by a **runtime** capability probe (not a compile-time
   feature), so one prebuilt binary works against servers with or without the
   endpoint (ADR-0004). Loops until Ctrl-C.
9. **Record** — `speak record` captures the microphone to a file
   (`--output`, `--device`, `--format wav|flac`, `--duration`, `--sample-rate`,
   `--channels`).
10. **Devices** — `speak devices [--json]` lists input and output audio devices
    (CoreAudio enumeration), including the `AudioDeviceID`s used by
    `--output-device` and `[audio.*].device`.
11. **Multi-output routing** — `--output-device` is repeatable on `say` and
    `realtime`; one device pins one engine, many devices fan one decode out to N
    engines (or an aggregate device), fully digital, no exec (ADR-0007).
12. **Daemon** — `speak daemon [--foreground|stop|status|restart]` runs a process
    that holds one warm pooled async-openai client and listens on a Unix socket
    (`[daemon].socket`, default `~/.speak/speak.sock`); CLI commands forward to
    it (length-prefixed framing, SSE frames streamed through) with transparent
    one-shot fallback when no daemon is present (ADR-0005). The daemon is
    SINGLE-INSTANCE and is only ever spawned by `speak` itself (ADR-0010): a PID
    file at `[daemon].pidfile` (`SPEAK_DAEMON_PIDFILE`, default
    `~/.speak/speak.pid`) records the owner, written atomically and removed on
    graceful shutdown (SIGINT/SIGTERM). Running `speak daemon` again REPLACES the
    previous instance — SIGTERM, wait up to `[daemon].kill_grace_ms`
    (`SPEAK_DAEMON_KILL_GRACE_MS`, default 3000), then SIGKILL — and cleans the
    leftover socket; a dead/stale pidfile is cleaned. `stop` SIGTERMs the pidfile
    PID and cleans up; `restart` is a replacing start; `status` reports
    running/pid/uptime/socket + upstream health through the Presenter (`--json`).
    A background WATCHDOG probes the upstream `/health` every
    `SPEAK_DAEMON_HEALTH_INTERVAL` (default 15s, 0 = off) with a
    `SPEAK_HEALTH_TIMEOUT` (default 5s) bound; after `SPEAK_DAEMON_HEALTH_FAILS`
    (default 3) consecutive failures it self-recovers (rebuild the warm pool,
    re-probe the realtime capability) and reports state in `status`, backing off
    through `[retry]` while degraded. A forwarded non-English `translate`/realtime
    request honours `--to` because the daemon's warm Facade holds the SAME
    in-process speech composite (chat-MT Strategy) as the CLI (ADR-0010).
    `[daemon].autostart` controls the one-shot fallback: when `true`, a one-shot
    invocation that finds no daemon auto-launches the daemon binary (detached),
    waits for the socket, and forwards to it; when `false` (default) it just runs
    the one-shot client.
13. **Config precedence and catalog** — `CLI flag > ENV (SPEAK_*) >
    ~/.speak/config.toml > code default` (ADR-0006). The file migrates from
    `~/.config/speak` if present. `config init` writes a fully-commented TOML;
    `config show` prints every effective value AND its origin
    (`flag|env|toml|default`); `config path` prints the resolved path. The full
    catalog of sections/keys is defined in ADR-0006 and the CUE schema, and
    includes a `[retry]` resilience section (FR-17) and an `[http]` section
    (`translate_url`, `translate_model`, `save_dir`).
14. **Health / models** — `speak health` checks `GET /health`; the client also
    reads `GET /v1/models`. `speak check` reports OS/arch, CPU cores, libav
    hwdevice types, AudioToolbox decoders, and the active acceleration policy
    (`SPEAK_HWACCEL=auto|off|<decoder>`); rotating logs go under `~/.speak/logs`
    (`SPEAK_LOG`, `SPEAK_LOG_DIR`; `SPEAK_LOG=off` disables) — ADR-0002.
15. **Completions** — `speak completions <shell>` emits a shell completion script.
16. **Compatibility & output** — works against any OpenAI-audio-compatible server
    via `--host`; honours `--quiet` and `--json` where applicable; returns a
    non-zero exit on HTTP/transport error.
17. **Resilience (retry/backoff)** — every network call (typed OpenAI endpoint,
    `_byot` speech request, chat-MT request, daemon forward, and the realtime
    SSE stream) is routed through a single configurable retry policy: bounded
    exponential backoff with jitter, controlled by the `[retry]` catalog
    (`max_retries`=`SPEAK_RETRY_MAX` default 3, `backoff_initial_ms`=
    `SPEAK_RETRY_BACKOFF_MS` 200, `backoff_max_ms`=`SPEAK_RETRY_BACKOFF_MAX_MS`
    5000, `multiplier`=`SPEAK_RETRY_MULTIPLIER` 2.0, `jitter`=
    `SPEAK_RETRY_JITTER` true, `retry_on`=`SPEAK_RETRY_ON`, default
    `connect + timeout + 5xx + 429`). Only failures in the `retry_on` set are
    retried; others fail fast. The realtime SSE stream reconnects under the same
    bounded policy. The policy is a domain value object behind a `RetryPolicy`
    port (Strategy), injected at the composition root and unit-tested (attempt
    count, delay growth, jitter bounds, `retry_on` classification) — ADR-0004.
18. **Universal env-overridability (no magic numbers)** — every tunable
    (timeouts, pool sizes, chunk sizes, buffer frames, silence thresholds,
    sample rates, ffmpeg knobs, and the full retry policy) is overridable via a
    `SPEAK_*` environment variable with a code default under the FR-13
    precedence, appears in `config show` with its origin, and is recorded in the
    ADR-0006 catalog and the CUE schema. No tunable is a hardcoded literal.

## Non-Functional Requirements

- **Architecture** — Hexagonal (Ports & Adapters) + DDD + named GoF patterns
  (ADR-0003). Dependencies point inward (`adapters -> application -> domain`);
  the domain is pure (zero I/O). Layout: `src/domain`, `src/ports`,
  `src/application`, `src/adapters/{openai,coreaudio,libav,config,daemon,sse}`,
  `src/cli`, `src/main.rs` (composition root / Factory).
- **HTTP client** — `async-openai` 0.41.x configured with
  `OpenAIConfig::with_api_base(host).with_api_key(key)`; typed requests for
  standard endpoints, `_byot` methods for the extended speech request
  (voice-design, clone, `ref_text`, gen-params); `eventsource-stream` for SSE
  (ADR-0004). One warm, pooled client reused across every request, including
  each realtime iteration.
- **In-process media, no exec** (ADR-0001, Constitution Principle 5) — server
  audio is decoded and resampled with linked `libav*` (ffmpeg-the-third 5.0.0 +
  ffmpeg 8.1, libavcodec 62) via a custom in-memory AVIO callback; the mic WAV
  is hand-muxed. Playback and capture use the native macOS CoreAudio mixer
  (`AVAudioEngine`: `AVAudioPlayerNode -> mainMixerNode -> outputNode`;
  `inputNode` tap for capture) via `objc2-avf-audio`. No
  `ffmpeg`/`ffplay`/`afplay`/`cpal`/`ffprobe`.
- **Platform** — native device I/O is macOS arm64 today; on other platforms
  playback/capture/record/devices return a clear error while file-oriented
  commands (transcribe, translate, say with `-o`, voices, health, config) keep
  working.
- **Async I/O** — tokio runtime; streaming request/response bodies where the
  server supports them.
- **Single self-contained binary**; config optional (sane defaults work out of
  the box).
- **Latency** — a `say` round trip should be dominated by server inference, not
  the client; the daemon and warm pool remove repeated connection setup.
- **Resilience** (FR-17, ADR-0004) — a single `RetryPolicy` Strategy (domain
  value object + port) wraps every network call with bounded exponential backoff
  + jitter; it is configured from `[retry]`, injected at the composition root,
  and unit-tested independently of any transport. Realtime SSE failures trigger
  a bounded reconnect rather than aborting the session.
- **Zero magic numbers** (FR-18) — every tunable is an env-overridable
  `SPEAK_*` knob with a code default; the Validate phase asserts (grep/review)
  that no tunable is a hardcoded literal and that the retry policy is env-driven
  and tested.
- **Output & logging discipline** (ADR-0009) — command RESULTS go to stdout
  through a swappable `Presenter` port (`console | json | buffer`; honours
  `--quiet`, `--json` per FR-16, and `--color`/`NO_COLOR`); DIAGNOSTICS go to
  stderr (gated by a `-v`/`--verbose` count + `RUST_LOG`/`SPEAK_LOG`) and ALWAYS
  to the rotating `~/.speak/logs` file through `tracing` (ADR-0002). No raw
  `println!`/`eprintln!` for program output; the `Presenter` is unit-testable via
  a capture buffer.

## Acceptance Scenarios

Given a reachable server at `$SPEAK_HOST`
When I run `speak say "olá"`
Then the server synthesizes pt-BR audio and it plays locally with exit code 0.

Given a reachable server
When I run `speak say "hi" --instruct "Female, Young Adult, British Accent"`
Then the server applies the voice design and the audio plays with exit code 0.

Given an audio file `a.mp3`
When I run `speak transcribe a.mp3 --format text`
Then stdout is the transcript text and exit code is 0.

Given foreign-language audio `f.mp3`
When I run `speak translate f.mp3`
Then stdout is the English translation.

Given a microphone
When I run `speak realtime --from en --to pt-BR --translate`
Then each utterance is transcribed, translated to pt-BR, printed, and spoken,
until Ctrl-C.

Given a microphone and two output devices
When I run `speak say "test" --output-device A --output-device B`
Then the decoded audio plays simultaneously on both devices.

Given no config file and no flags
When I run `speak say "oi"`
Then it uses the default host `http://solaris:8800` and succeeds.

Given a value set in the TOML and overridden by an env var
When I run `speak config show`
Then each effective value is printed with its origin (`flag|env|toml|default`).

Given a server that returns a transient 5xx then succeeds
When I run `speak say "oi"`
Then the client retries with exponential backoff + jitter (per `[retry]`) and
the request ultimately succeeds with exit code 0.

Given a server that returns a non-retryable 4xx
When I run `speak say "oi"`
Then the client does not retry and exits non-zero with the server error.

## Out of Scope

- Hosting/serving models (the server already does that).
- A GUI; bundling or shelling out to ffmpeg/afplay/ffplay (all media is linked
  in-process).
- Arbitrary text machine-translation without an LLM endpoint (realtime uses
  Whisper translate -> English by default; an arbitrary target requires
  `translate_url`/`translate_model`).
- Windows/Linux native audio device I/O (future work); file commands still run.
