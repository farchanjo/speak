# Tasks: speak

Layer tags map each task to the hexagonal layer it belongs to (ADR-0003):
`[domain]`, `[ports]`, `[application]`, `[adapter:*]`, `[cli]`, `[root]`,
`[cross]`, `[build]`, `[docs]`. Order respects inward dependency flow
(`domain -> ports -> adapters -> application -> driving adapters -> root`).
`[x]` = present in the current tree (may need to move/refactor into the layered
layout); `[ ]` = pending for the hexagonal rebuild. The flat-layout client
(`say`/`transcribe`/`translate`/`realtime`/`voices`/`daemon`/`check`/`health`/
`config`/`completions`) is the shipped behavior behind the speckit
`implemented` marker; the layered tree, the `async-openai` migration, the
`record`/`devices` commands, multi-output fan-out, the realtime-mode flags, the
`ServerProbe` capability/health port (T022), the `check`/`health` use case
(T047), and the edition-2024 bump are the in-progress refactor tracked below
(see spec.md "Implementation Status").

## Foundation (build + cross-cutting)

- [x] T001 `[build]` Cargo manifest: tokio, clap+clap_complete, serde/serde_json,
  toml, anyhow, tracing(+appender), ffmpeg-the-third, objc2/objc2-foundation/
  objc2-avf-audio/block2; `[profile.release]` lto/strip/codegen-units=1.
- [ ] T002 `[build]` Add `async-openai` 0.41.x and `eventsource-stream`; plan the
  migration off direct `reqwest` use to the async-openai client (keep
  `reqwest` only transitively).
- [x] T003 `[cross]` `accel`: OS/arch probe, libav hwdevice + AudioToolbox
  decoders, `SPEAK_HWACCEL=auto|off|<decoder>` policy, frame threading
  (ADR-0002).
- [x] T004 `[cross]` `logging`: tracing daily-rotating logs under `~/.speak/logs`
  (`SPEAK_LOG`/`SPEAK_LOG_DIR`, retention cap, non-blocking) (ADR-0002).

## Domain (pure, zero IO)

- [ ] T010 `[domain]` `Language`, `SampleFormat`, `PcmBuffer` value objects.
- [x] T011 `[domain]` `VoiceDesign` value object: the canonical 23 EN tags with
  parse/validate (reject free text) and `list-designs` source.
  (`src/domain/voice_design.rs`, unit-tested; wired into `say --instruct` and
  `--list-designs`.)
- [ ] T012 `[domain]` `Voice`, `VoiceClone` (saved name, optional `ref_text`).
- [x] T013 `[domain]` `GenParams` value object (num_step/steps alias,
  guidance_scale, t_shift, layer_penalty_factor, position/class_temperature,
  denoise, preprocess_prompt, postprocess_output, audio_chunk_duration/
  threshold) with validated key set. The only canonical step key is `num_step`
  (CLI alias `steps`); reject `num_steps` and any other unknown key.
  (`src/domain/gen_params.rs`, unit-tested; wired into `say --set`.)
- [ ] T014 `[domain]` `SpeechSpec` aggregate (input + voice mode + format +
  language + speed + gen-params) and domain `errors`.
- [x] T015 `[domain]` `RetryPolicy` value object (max_retries, backoff_initial_ms,
  backoff_max_ms, multiplier, jitter, `retry_on` classification via `RetryOn`):
  pure backoff/jitter computation, no I/O (FR-17 / ADR-0004). Unit-test attempt
  count, geometric delay growth, jitter bounds, and `retry_on` classification.
  (`src/domain/retry.rs`, six unit tests; seed-injected jitter for purity.)

## Ports (traits)

- [ ] T020 `[ports]` `Synthesizer`, `Transcriber`, `Translator`.
- [ ] T021 `[ports]` `AudioSink` (single + multi-device), `AudioSource`,
  `AudioDecoder`, `AudioEncoder` (WAV/FLAC record output).
- [ ] T022 `[ports]` `ConfigProvider`, `VoiceRepository`, `RealtimeStream`, and
  `ServerProbe` (the capability/health port covering `GET /health`,
  `GET /v1/models`, and the runtime `POST /v1/realtime/translate` probe of
  FR-14 / ADR-0004 — the network calls behind `speak health`/`check` and the
  SSE-vs-chunked selection).
- [ ] T023 `[ports]` `RetryPolicy` port (the resilience Strategy the composition
  root injects around every network call) (FR-17 / ADR-0004).

## Driven adapters

- [ ] T030 `[adapter:openai]` async-openai client (`with_api_base`/`with_api_key`);
  typed requests for `/v1/models`, `/v1/audio/transcriptions`,
  `/v1/audio/translations`, voice CRUD (ADR-0004).
- [ ] T031 `[adapter:openai]` `_byot` extended `/v1/audio/speech` + native `/tts`
  request via a fluent **Builder** (voice-design, clone, ref_text, gen-params);
  implements `Synthesizer`.
- [ ] T032 `[adapter:openai]` `VoiceRepository` over `POST/GET/DELETE /voices`
  (multipart).
- [x] T033 `[adapter:libav]` custom in-memory AVIO decode -> PCM, libswresample
  resample (48 kHz stereo f32 / 16 kHz mono s16), in-memory WAV mux, RMS gate;
  implements `AudioDecoder` (ADR-0001).
- [x] T034 `[adapter:coreaudio]` `AVAudioEngine` playback
  (`AVAudioPlayerNode -> mainMixerNode -> outputNode`) + `inputNode` capture;
  implements `AudioSink`/`AudioSource` (ADR-0001).
- [ ] T035 `[adapter:coreaudio]` device enumeration
  (`kAudioHardwarePropertyDevices`) for `speak devices`; multi-output fan-out to
  N engines / aggregate device, volume -> `mainMixerNode.outputVolume`
  (ADR-0007).
- [ ] T036 `[adapter:sse]` `eventsource-stream` consumer decoding realtime frames
  `{type, text?, audio_b64?, format?, seq?}` into a typed `RealtimeFrame`;
  implements `RealtimeStream`. Selection is a **runtime** capability probe (via
  `ServerProbe`), not a compile-time feature: one prebuilt binary detects the
  endpoint at run time and falls back to the chunked ASR->MT->TTS loop when it
  is absent (the `eventsource-stream` dependency is always linked; an optional
  `realtime-sse` Cargo feature may gate the parser out only for size-constrained
  builds) (ADR-0004).
- [ ] T037 `[adapter:config]` layered config (flags > env > `~/.speak/config.toml`
  > default), migration from `~/.config/speak`, `config init` commented template,
  `config show` value+origin, `config path`; implements `ConfigProvider`
  (ADR-0006). Load the `[retry]` section (mapped to `domain::RetryPolicy`) and
  the `[http]` section (`translate_url`/`translate_model`/`save_dir`), each with
  its `SPEAK_*` env override and code default — no hardcoded tunables (FR-18).
  During this rebuild, rename the `[tts.gen]` `gen` field to the
  `domain::GenParams` value object and bump `edition = "2024"`/`resolver = "3"`/
  `rust-version = "1.95"`, adding a `rust-toolchain.toml` (`channel = "1.95"`)
  (the deferral owner, ADR-0008): run `cargo fix --edition`, verify MSRV 1.95
  (`cargo msrv verify`) and a green build/clippy.
  (Partial: the `[retry]` section now resolves into `domain::retry::RetryPolicy`
  with full `SPEAK_RETRY_*` overrides and appears in `config show`; the layered
  tree move, the `[http]` split, and the edition-2024 bump remain pending.)
- [ ] T038 `[adapter:libav]` record-output **encode**: hand-muxed WAV (no
  encoder) and FLAC via the libavcodec FLAC encoder through an in-memory AVIO
  **write** callback; implements `AudioEncoder` (`record --format wav|flac`,
  FR-9 / ADR-0001).
- [ ] T039 `[adapter:chatmt]` arbitrary-target machine translation: implement the
  `Translator` **Strategy** against `[http].translate_url` /
  `[http].translate_model` (non-OpenAI chat-MT endpoint), reusing the warm pool;
  selected when `--to` is non-English and `translate_url` is set, else degrade
  to the source transcript with a clear notice (FR-8 / ADR-0004).
- [ ] T046 `[adapter:retry]` transport-agnostic retry **decorator(s)**: for each
  wrapped driven port (`Synthesizer`, `Transcriber`, `Translator`,
  `VoiceRepository`, `RealtimeStream`, `ServerProbe`) a generic
  decorator that **implements that same port** (so it is substitutable for the
  concrete adapter) and consults the injected `RetryPolicy` **Strategy** (T023,
  driven by `domain::RetryPolicy`) for the backoff schedule. The decorator is
  NOT itself the `RetryPolicy` port; it is a port-preserving wrapper that calls
  the policy. Bounded exponential backoff + jitter, `retry_on` classification
  (connect/timeout/5xx/429); the `sse` reconnect rides the same policy. The
  CLI-side daemon-forward path is not a driven port: its decorator preserves the
  application **Facade** surface (`CommandTransport`, ADR-0005), retrying the
  socket connect/forward under the same Strategy.
  Configured from `[retry]`, injected at the composition root (FR-17 / ADR-0004).
  (Partial: the seeded backoff loop now lives in the reqwest `SpeechClient` and
  honors the full `retry_on` classification incl. 5xx/429 responses; extraction
  into a generic port-preserving decorator across all driven ports is pending.)

## Application (use cases)

- [ ] T040 `[application]` `say` use case (TTS: voice modes, format, gen-params,
  play vs `-o`/`--no-play`, multi-output). A bare `-o` filename resolves under
  `[http].save_dir` (`SPEAK_SAVE_DIR`, default CWD); on `--json` surface the
  server's `X-RTF`/`X-Audio-Seconds` headers when present (FR-1).
- [ ] T041 `[application]` `transcribe` and `translate` (file) use cases.
- [ ] T042 `[application]` `voices` use case (add/list/rm via `VoiceRepository`).
- [ ] T043 `[application]` `record` use case (capture -> WAV/FLAC file).
- [ ] T044 `[application]` `realtime` use case with the three **Strategy** modes
  (`translate`/`no-translate`/`echo`), SSE or chunked, multi-output.
- [ ] T045 `[application]` application **Facade** shared by CLI and daemon.
- [ ] T047 `[application]` `check`/`health` use case: orchestrate the `ServerProbe`
  port (`GET /health`, `GET /v1/models`, realtime capability probe) and the
  `accel` cross-cutting probe into the data printed by `speak health` and
  `speak check` (FR-14). `config`, `devices`, and `completions` remain thin CLI
  adapters that read the `ConfigProvider`/device-enumeration adapters directly
  and need no dedicated use case.

## Driving adapters + composition root

- [x] T050 `[cli]` clap CLI surface present in the current tree: `say|tts`,
  `transcribe`, `translate`, `realtime`, `voices`, `daemon`, `check`, `health`,
  `config`, `completions`; global flags with `env=`; `ValueEnum` choices. The
  `record` and `devices` commands, the repeatable `--output-device`, the
  per-call `say --voice` / `realtime --voice`, the `--translate`/
  `--no-translate`/`--echo` realtime modes (replacing the current
  `--repeat`/`--speak`), `--list-designs`, and the global `--json` flag are NOT
  yet present and are added by T051/T055/T056.
- [ ] T051 `[cli]` wire each subcommand to its use case (no business logic in
  the CLI); add the repeatable `--output-device` on `say`/`realtime`; `say
  --voice` (per-call clone, distinct from the TTS `--voice`/`alloy`), `realtime
  --instruct/--voice` (output voice), `realtime --translate/--no-translate/
  --echo` modes; `--list-designs`; the global `--json` flag (FR-16).
- [x] T052 `[adapter:daemon]` Unix-socket listener at `~/.speak/speak.sock`,
  length-prefixed framing, SSE pass-through, one-shot fallback (ADR-0005).
- [ ] T053 `[adapter:daemon]` route framed requests through the shared
  application Facade (same use cases as the CLI).
- [ ] T054 `[root]` `main.rs` composition root (**Factory**/DI): build the one
  warm async-openai client, construct the `RetryPolicy` from `[retry]` and wrap
  every network adapter with its port-preserving retry decorator (T046), wire
  the `check`/`health` use case to the `ServerProbe` adapter, wire all adapters
  into use cases, select CLI vs daemon.
- [ ] T055 `[cli]` wire `speak record` (`--output`, `--device`, `--format
  wav|flac`, `--duration`, `--sample-rate`, `--channels`) to the `record` use
  case (FR-9).
- [ ] T056 `[cli]` wire `speak devices [--json]` to the device-enumeration
  adapter (T035) and print input/output devices + `AudioDeviceID`s (FR-10).

## Verification

- [ ] T060 `[build]` `cargo build --release` GREEN; `cargo clippy --all-targets
  -- -D warnings`; `cargo fmt --all -- --check`; `cargo test`/`nextest`.
- [ ] T063 `[build]` Resilience + zero-magic-numbers gate (FR-17 / FR-18):
  unit-test the `RetryPolicy` (attempt count, delay growth, jitter bounds,
  `retry_on` classification) and assert via grep/review that no tunable
  (timeouts, pool sizes, chunk/buffer sizes, thresholds, sample rates, ffmpeg
  knobs, retry params) is a hardcoded literal — every one resolves through the
  `SPEAK_*` env override + code default and appears in `config show`.
- [ ] T061 `[docs]` smoke-test `health`, `say`, `transcribe`/`translate`,
  `realtime`, `devices`, `daemon` against solaris; update README + quickstart.
- [ ] T062 `[docs]` `speckit validate` + `speckit analyze` clean; `speckit verify`
  Gherkin corpus; commit docs together.

## Dependencies

- Reachable server `http://solaris:8800` (OmniVoice + Whisper), OpenAI-compatible
  v2.3, plus the optional SSE endpoint `POST /v1/realtime/translate`.
- macOS arm64 + Homebrew ffmpeg 8.1 (libavcodec 62) + LLVM (libclang); Rust 1.95.
- Build env: `PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig`,
  `LIBCLANG_PATH=/opt/homebrew/opt/llvm/lib`.
