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
- [x] T002 `[build]` Add `async-openai` 0.41.x and `eventsource-stream`; plan the
  migration off direct `reqwest` use to the async-openai client (keep
  `reqwest` only transitively).
  (`async-openai = 0.41` with `default-features = false, features = ["rustls"]`
  and `eventsource-stream = 0.2` are now manifest dependencies and build green;
  they are not yet wired — the flat `reqwest` `SpeechClient` stays in place until
  the `openai`/`sse` adapters land in a later stage.)
- [x] T003 `[cross]` `accel`: OS/arch probe, libav hwdevice + AudioToolbox
  decoders, `SPEAK_HWACCEL=auto|off|<decoder>` policy, frame threading
  (ADR-0002).
- [x] T004 `[cross]` `logging`: tracing daily-rotating logs under `~/.speak/logs`
  (`SPEAK_LOG`/`SPEAK_LOG_DIR`, retention cap, non-blocking) (ADR-0002).

## Domain (pure, zero IO)

- [x] T010 `[domain]` `Language`, `SampleFormat`, `PcmBuffer` value objects.
  (`src/domain/language.rs`: normalized tag + English detection;
  `src/domain/pcm.rs`: `SampleFormat` widths + interleaved-f32 `PcmBuffer` with
  frame/duration arithmetic. Pure, unit-tested.)
- [x] T011 `[domain]` `VoiceDesign` value object: the canonical 23 EN tags with
  parse/validate (reject free text) and `list-designs` source.
  (`src/domain/voice_design.rs`, unit-tested; wired into `say --instruct` and
  `--list-designs`.)
- [x] T012 `[domain]` `Voice`, `VoiceClone` (saved name, optional `ref_text`).
  (`src/domain/voice.rs`: `Voice` (name + ref-text flag), `VoiceClone` (name +
  normalized optional `ref_text`), `StandardVoice` (the `alloy` default), and
  the three-arm `VoiceMode` Strategy selector. Pure, unit-tested.)
- [x] T013 `[domain]` `GenParams` value object (num_step/steps alias,
  guidance_scale, t_shift, layer_penalty_factor, position/class_temperature,
  denoise, preprocess_prompt, postprocess_output, audio_chunk_duration/
  threshold) with validated key set. The only canonical step key is `num_step`
  (CLI alias `steps`); reject `num_steps` and any other unknown key.
  (`src/domain/gen_params.rs`, unit-tested; wired into `say --set`.)
- [x] T014 `[domain]` `SpeechSpec` aggregate (input + voice mode + format +
  language + speed + gen-params) and domain `errors`.
  (`src/domain/speech_spec.rs`: immutable aggregate assembled via a fluent
  Builder enforcing non-empty input, positive/finite speed, and a chosen voice
  mode + language; `src/domain/audio_format.rs`: the `mp3|opus|aac|flac|wav|pcm`
  `AudioFormat`; `src/domain/realtime.rs`: the `RealtimeMode` Strategy;
  `src/domain/errors.rs`: the pure `DomainError` enum (impls `std::error::Error`
  for the anyhow bridge). All unit-tested.)
- [x] T015 `[domain]` `RetryPolicy` value object (max_retries, backoff_initial_ms,
  backoff_max_ms, multiplier, jitter, `retry_on` classification via `RetryOn`):
  pure backoff/jitter computation, no I/O (FR-17 / ADR-0004). Unit-test attempt
  count, geometric delay growth, jitter bounds, and `retry_on` classification.
  (`src/domain/retry.rs`, six unit tests; seed-injected jitter for purity.)

## Ports (traits)

- [x] T020 `[ports]` `Synthesizer`, `Transcriber`, `Translator`.
  (`src/ports/{synthesizer,transcriber,translator}.rs`: `Synthesizer` consumes
  the `SpeechSpec` aggregate and returns `SynthesizedAudio` (bytes + `X-RTF`/
  `X-Audio-Seconds`); `Translator` is the two-Strategy port. Async ports; no
  framework type in any signature.)
- [x] T021 `[ports]` `AudioSink` (single + multi-device), `AudioSource`,
  `AudioDecoder`, `AudioEncoder` (WAV/FLAC record output).
  (`src/ports/audio.rs`: `AudioSink` with `play`/`play_to` fan-out (FR-11) +
  `AudioDevice` enumeration, `AudioSource` capture + input enumeration;
  `src/ports/codec.rs`: `AudioDecoder` (decode + resample) and `AudioEncoder`
  (`RecordFormat::Wav|Flac`).)
- [x] T022 `[ports]` `ConfigProvider`, `VoiceRepository`, `RealtimeStream`, and
  `ServerProbe` (the capability/health port covering `GET /health`,
  `GET /v1/models`, and the runtime `POST /v1/realtime/translate` probe of
  FR-14 / ADR-0004 — the network calls behind `speak health`/`check` and the
  SSE-vs-chunked selection).
  (`src/ports/{config,voice,realtime,probe}.rs`: `VoiceRepository` (add/list/rm),
  `RealtimeStream` yielding a typed `RealtimeFrame`, and `ServerProbe`
  (health/models/supports_realtime). `ConfigProvider` returns the resolved POD
  `config::Config`, which moves inward when the config adapter lands.)
- [x] T023 `[ports]` `RetryPolicy` port (the resilience Strategy the composition
  root injects around every network call) (FR-17 / ADR-0004).
  (`src/ports/retry.rs`: the Strategy port with a blanket impl for the pure
  `domain::retry::RetryPolicy` value object, exercised via dynamic dispatch in a
  unit test.)

## Driven adapters

- [x] T030 `[adapter:openai]` async-openai client (`with_api_base`/`with_api_key`);
  typed requests for `/v1/models`, `/v1/audio/transcriptions`,
  `/v1/audio/translations`, voice CRUD (ADR-0004).
  (`src/adapters/openai/`: `OpenAiAdapter::new` is the Factory — it builds the
  `async-openai` `Client<OpenAIConfig>` from `OpenAIConfig::default()
  .with_api_base("{host}/v1").with_api_key(...)` and a tuned warm `reqwest` pool
  (`client::build_http_client`, extracted + shared with the flat `SpeechClient`).
  `Transcriber`/`Translator` drive the typed `audio().transcription()` /
  `audio().translation()` groups via `create_raw` so every `response_format`
  round-trips as bytes. async-openai 0.41 links `reqwest` 0.13 while this crate
  is on 0.12, so the two `Client`s cannot share one instance — unifying the pool
  is a composition-root concern (T054). The `audio` Cargo feature was added.)
- [x] T031 `[adapter:openai]` `_byot` extended `/v1/audio/speech` + native `/tts`
  request via a fluent **Builder** (voice-design, clone, ref_text, gen-params);
  implements `Synthesizer`.
  (`src/adapters/openai/speech.rs`: async-openai 0.41 exposes no non-streaming
  speech "bring-your-own-types" method and `Speech::create` discards the
  `X-RTF`/`X-Audio-Seconds` headers FR-1 needs, so the Synthesizer serializes a
  `speak`-owned `SpeechBody` — built by the fluent `SpeechBodyBuilder`, mapping
  the domain `VoiceMode` Strategy to `instruct`/`voice`/`ref_text` and flattening
  the gen-params — and posts it over the tuned warm pool, collecting the bytes +
  timing headers. `--native` routes to the `/tts` body (text/language/speed).)
- [x] T032 `[adapter:openai]` `VoiceRepository` over `POST/GET/DELETE /voices`
  (multipart).
  (`src/adapters/openai/voices.rs`: the server's `/voices` surface is not the
  OpenAI `/audio/voices` endpoint, so the Repository posts multipart
  `name,audio,ref_text?` to `POST /voices`, parses the `{voices:[{name,
  has_ref_text}]}` envelope into `domain::voice::Voice`, and `DELETE`s by name —
  all over the same warm pool.)
- [x] T033 `[adapter:libav]` custom in-memory AVIO decode -> PCM, libswresample
  resample (48 kHz stereo f32 / 16 kHz mono s16), in-memory WAV mux, RMS gate;
  implements `AudioDecoder` (ADR-0001).
  (The libav FFI moved out of the flat `src/codec.rs` into the
  `src/adapters/libav/` driven adapter — now the ONLY place `ffmpeg-the-third`
  appears. `LibavCodec` (Factory `new(DecodeOptions)`) implements
  `ports::AudioDecoder`: `decode` produces the pure `domain::pcm::PcmBuffer`
  (the flat `codec::Pcm` duplicate is gone) and `resample` is the general
  FLT->FLT path; the canonical-rate/channel constants, `to_asr_mono16`,
  `wav_mono16` and `rms_s16` are re-exported for the still-flat realtime path
  until T044/T055. No `ffmpeg` type crosses the port boundary.)
- [x] T034 `[adapter:coreaudio]` `AVAudioEngine` playback
  (`AVAudioPlayerNode -> mainMixerNode -> outputNode`) + `inputNode` capture;
  implements `AudioSink`/`AudioSource` (ADR-0001).
  (The AVFAudio FFI moved out of the flat `src/audio_macos.rs`/`audio_stub.rs`
  into the `src/adapters/coreaudio/` driven adapter — now the ONLY place `objc2`
  appears. `CoreAudio` (Factory `new`) implements `ports::AudioSink::play` and
  `ports::AudioSource::capture` over the pure `domain::pcm::PcmBuffer`, off-loaded
  to `spawn_blocking`; a macOS backend is cfg-gated against a clear-error stub.
  The `play`/`capture_chunk` free fns are re-exported for the still-flat realtime
  path until T044.)
- [x] T035 `[adapter:coreaudio]` device enumeration
  (`kAudioHardwarePropertyDevices`) for `speak devices`; multi-output fan-out to
  N engines / aggregate device, volume -> `mainMixerNode.outputVolume`
  (ADR-0007).
  (`src/adapters/coreaudio/macos/device.rs`: CoreAudio HAL enumeration walks the
  system object's `kAudioHardwarePropertyDevices`, reading each device's name +
  UID (`CFString`), per-direction channel counts (`AudioBufferList` over
  `StreamConfiguration`), nominal sample rate, and default-in/out status into the
  port `AudioDevice` (extended with `uid`/`sample_rate`/`is_default_*` for FR-10);
  `AudioSink::outputs`/`AudioSource::inputs` filter it by direction. Multi-output
  fan-out (`play_to`) builds one `AVAudioEngine` per target, pinning each output
  unit to its `AudioDeviceID` via `AudioUnitSetProperty(CurrentDevice)` and
  scheduling the same decoded buffer on each (one decode -> N devices); volume
  maps to `mainMixerNode.outputVolume`. Live-verified against this host's 9 HAL
  devices (names/UIDs/channels/rates/defaults correct). The `speak devices` CLI
  wiring is T056.)
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
  with full `SPEAK_RETRY_*` overrides and appears in `config show`; the
  edition-2024 / resolver-3 / rust-version-1.95 bump + `rust-toolchain.toml`
  landed (ADR-0008 resolved), including the `[tts.gen]` -> `gen_params` rename
  (serde keeps the TOML key) and the let-chain / `unsafe`-block fixups; the
  layered tree move and the `[http]` split remain pending.)
- [x] T038 `[adapter:libav]` record-output **encode**: hand-muxed WAV (no
  encoder) and FLAC via the libavcodec FLAC encoder through an in-memory AVIO
  **write** callback; implements `AudioEncoder` (`record --format wav|flac`,
  FR-9 / ADR-0001).
  (`src/adapters/libav/encode.rs`: `encode_wav` quantises the captured
  `PcmBuffer` (f32) to interleaved s16 and hand-muxes a multi-channel RIFF/WAVE
  buffer; `encode_flac` drives the libavcodec FLAC encoder and muxes the `.flac`
  container through a custom in-memory AVIO **write** callback — the mirror of
  the decode read callback — with a seekable `MemSink` so the muxer back-patches
  STREAMINFO; no temp files, no exec. `LibavCodec` implements
  `ports::AudioEncoder::encode` over the `RecordFormat` Strategy. Validated by an
  encode->decode round-trip unit test (mono + stereo); the `record` CLI/use case
  wiring is T043/T055.)
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

- [x] T040 `[application]` `say` use case (TTS: voice modes, format, gen-params,
  play vs `-o`/`--no-play`, multi-output). A bare `-o` filename resolves under
  `[http].save_dir` (`SPEAK_SAVE_DIR`, default CWD); on `--json` surface the
  server's `X-RTF`/`X-Audio-Seconds` headers when present (FR-1).
  (`src/application/say.rs`: `SayUseCase` is generic over the `Synthesizer`,
  `AudioDecoder`, and `AudioSink` ports — it synthesizes the validated
  `SpeechSpec`, and unless `SayOptions.play` is false decodes the bytes and
  routes them to the default device or fans them out to N `--output-device`s via
  the shared `application::playback` helper (FR-11). The `SayOutcome` returns the
  encoded bytes + `X-RTF`/`X-Audio-Seconds` so the driving adapter honours
  `-o`/`--json` (the save-path resolution + file write stay in the CLI/daemon
  driving adapter, T051/T053). Unit-tested over the in-memory port doubles. The
  CLI wiring onto this use case is T051.)
- [x] T041 `[application]` `transcribe` and `translate` (file) use cases.
  (`src/application/transcribe.rs` + `src/application/translate.rs`:
  `TranscribeUseCase` over the `Transcriber` port and `TranslateUseCase` over the
  `Translator` **Strategy** port are the thin shared seams the CLI and daemon
  both call (the root injects the openai/Whisper vs chat-MT translate strategy).
  Both unit-tested over the port doubles; CLI wiring is T051.)
- [x] T042 `[application]` `voices` use case (add/list/rm via `VoiceRepository`).
  (`src/application/voices.rs`: `VoicesUseCase` drives the `VoiceRepository`
  Repository port (`add`/`list`/`remove`); reading the reference audio file stays
  a driving-adapter concern (the use case receives the bytes). Unit-tested over
  the port doubles with an add->list->remove round-trip; CLI wiring is T051.)
- [x] T043 `[application]` `record` use case (capture -> WAV/FLAC file).
  (`src/application/record.rs`: `RecordUseCase` orchestrates `AudioSource`
  capture -> `AudioDecoder` resample (only when the requested `--sample-rate`/
  `--channels` differ from the device's) -> `AudioEncoder` WAV/FLAC mux (FR-9).
  `RecordOutcome` returns the muxed bytes + frames/secs so the driving adapter
  writes `--output`. Unit-tested over the port doubles (no-resample WAV and
  resampled FLAC paths); the `speak record` CLI wiring is T055.)
- [x] T044 `[application]` `realtime` use case with the three **Strategy** modes
  (`translate`/`no-translate`/`echo`), SSE or chunked, multi-output.
  (`src/application/realtime.rs`: `RealtimeUseCase` is generic over the three
  adapter roles (`Speech` = synthesize/transcribe/translate, `Audio` =
  capture/play, `Codec` = resample/encode). The chunked path `step()` captures
  one chunk, resamples to 16 kHz mono, applies the pure RMS silence gate, then
  dispatches the `RealtimeMode` Strategy — `Translate` (Whisper/chat-MT via the
  `Translator` port -> re-voice), `NoTranslate` (ASR -> re-voice), `Echo` (raw
  playback -> ASR -> re-voice); all re-voicing builds a per-chunk `SpeechSpec` in
  the chosen output voice and routes through the shared `playback` helper (single
  device or fan-out, FR-11). The SSE path `pump_frame()`/`drive_stream()` consume
  the `RealtimeStream` port's typed `RealtimeFrame`s (Audio -> decode+play, text
  -> surfaced to the driving adapter, Done/Error -> terminate); the runtime
  SSE-vs-chunked selection is the composition root's (T054). The Ctrl-C loop and
  terminal output stay in the driving adapter. Six unit tests over the port
  doubles cover all three modes, the VAD gate, empty results, and frame pumping;
  the realtime CLI flags (`--translate`/`--no-translate`/`--echo`,
  `--output-device`) are T051.)
- [x] T045 `[application]` application **Facade** shared by CLI and daemon.
  (`src/application/facade.rs`: `SpeakFacade` is generic over the three adapter
  roles the composition root injects (`Speech`/`Audio`/`Codec`) and owns them;
  every method (`say`/`transcribe`/`translate`/`add_voice`/`list_voices`/
  `remove_voice`/`record`/`realtime_step`/`realtime_frame`/`health`/`check`)
  builds the relevant use case from borrows and delegates, so the CLI and daemon
  driving adapters share one entry. Per-method `where` bounds keep it
  constructible even when a role lacks a port. Unit-tested end-to-end over the
  in-memory port doubles; the CLI/daemon wiring onto the Facade is T051/T053/T054.)
- [x] T047 `[application]` `check`/`health` use case: orchestrate the `ServerProbe`
  port (`GET /health`, `GET /v1/models`, realtime capability probe) and the
  `accel` cross-cutting probe into the data printed by `speak health` and
  `speak check` (FR-14). `config`, `devices`, and `completions` remain thin CLI
  adapters that read the `ConfigProvider`/device-enumeration adapters directly
  and need no dedicated use case.
  (`src/application/check.rs`: `CheckUseCase` drives the `ServerProbe` port —
  `health()` returns a `HealthOutcome` (healthy + advertised models + realtime
  capability, the realtime probe best-effort), and `check()` folds in the `accel`
  `Report` (passed as plain cross-cutting data per ADR-0003, never a port) into a
  `CheckOutcome`. The supporting `ServerProbe` adapter landed on the openai
  adapter (`src/adapters/openai/probe.rs`: `GET /health`, `GET /v1/models`
  parsing, and the `GET /v1/realtime/translate` runtime capability probe over the
  warm pool). Both unit-tested over the port doubles; the probe parsing has its
  own units. LIVE-VERIFIED against solaris: `/health` 200, `/v1/models` advertises
  omnivoice/tts-1/gpt-4o-mini-tts/whisper-1/large-v3, and the realtime endpoint
  returns 404 so `supports_realtime` correctly reports the chunked fallback. CLI
  wiring of `check`/`health` onto this use case is T054.)

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
  (Partial: `say`/`transcribe`/`translate`/`voices` now build the domain
  `SpeechSpec`/`TranscribeRequest`/`Language` and drive the `openai` adapter
  ports (`Synthesizer`/`Transcriber`/`Translator`/`VoiceRepository`) directly
  in-process, replacing the raw `Transport` proxy for these four commands and
  verified live against solaris (voice-design say + voices list). The
  application-layer use cases (T040-T042), the daemon-forward / Facade
  unification (T045/T053/T054), the per-call `--voice` clone, `--output-device`,
  realtime-mode flags, and the global `--json` remain pending; `realtime`,
  `health`, and the `daemon` server still use the flat `Transport`/`SpeechClient`
  for now. The `translate` command is English-only here (the `Translator` port is
  text-valued); subtitle output returns with the file-translate use case T041.)
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
- [x] T056 `[cli]` wire `speak devices [--json]` to the device-enumeration
  adapter (T035) and print input/output devices + `AudioDeviceID`s (FR-10).
  (`src/main.rs`: the `Devices` subcommand is a thin CLI adapter (per T047) that
  reads `coreaudio::enumerate()` directly and prints a starred-default table of
  output then input devices — `[id] name <channels>ch @ <rate> Hz uid=<UID>` —
  or, with `--json`, a JSON array carrying id/uid/name/input_channels/
  output_channels/sample_rate/default_input/default_output. Live-verified against
  this host (9 devices; SSL 12 starred as both default in + out). The global
  `--json` flag (FR-16) that will supersede this per-command flag is still T051.)

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
