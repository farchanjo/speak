# Tasks: speak

Layer tags map each task to the hexagonal layer it belongs to (ADR-0003):
`[domain]`, `[ports]`, `[application]`, `[adapter:*]`, `[cli]`, `[root]`,
`[cross]`, `[build]`, `[docs]`. Order respects inward dependency flow
(`domain -> ports -> adapters -> application -> driving adapters -> root`).
`[x]` = present in the current tree (may need to move/refactor into the layered
layout); `[ ]` = pending for the hexagonal rebuild.

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
- [ ] T011 `[domain]` `VoiceDesign` value object: the canonical 23 EN tags with
  parse/validate (reject free text) and `list-designs` source.
- [ ] T012 `[domain]` `Voice`, `VoiceClone` (saved name, optional `ref_text`).
- [ ] T013 `[domain]` `GenParams` value object (num_step/steps alias,
  guidance_scale, t_shift, layer_penalty_factor, position/class_temperature,
  denoise, preprocess_prompt, postprocess_output, audio_chunk_duration/
  threshold) with validated key set.
- [ ] T014 `[domain]` `SpeechSpec` aggregate (input + voice mode + format +
  language + speed + gen-params) and domain `errors`.

## Ports (traits)

- [ ] T020 `[ports]` `Synthesizer`, `Transcriber`, `Translator`.
- [ ] T021 `[ports]` `AudioSink` (single + multi-device), `AudioSource`,
  `AudioDecoder`.
- [ ] T022 `[ports]` `ConfigProvider`, `VoiceRepository`, `RealtimeStream`.

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
  implements `RealtimeStream`; feature-flag/stub when endpoint absent (ADR-0004).
- [x] T037 `[adapter:config]` layered config (flags > env > `~/.speak/config.toml`
  > default), migration from `~/.config/speak`, `config init` commented template,
  `config show` value+origin, `config path`; implements `ConfigProvider`
  (ADR-0006).

## Application (use cases)

- [ ] T040 `[application]` `say` use case (TTS: voice modes, format, gen-params,
  play vs `-o`/`--no-play`, multi-output).
- [ ] T041 `[application]` `transcribe` and `translate` (file) use cases.
- [ ] T042 `[application]` `voices` use case (add/list/rm via `VoiceRepository`).
- [ ] T043 `[application]` `record` use case (capture -> WAV/FLAC file).
- [ ] T044 `[application]` `realtime` use case with the three **Strategy** modes
  (`translate`/`no-translate`/`echo`), SSE or chunked, multi-output.
- [ ] T045 `[application]` application **Facade** shared by CLI and daemon.

## Driving adapters + composition root

- [x] T050 `[cli]` clap CLI surface: `say|tts`, `transcribe`, `translate`,
  `realtime`, `record`, `voices`, `devices`, `daemon`, `check`, `health`,
  `config`, `completions`; global flags with `env=`; `ValueEnum` choices.
- [ ] T051 `[cli]` wire each subcommand to its use case (no business logic in
  the CLI); `--output-device` repeatable on `say`/`realtime`; `--list-designs`.
- [x] T052 `[adapter:daemon]` Unix-socket listener at `~/.speak/speak.sock`,
  length-prefixed framing, SSE pass-through, one-shot fallback (ADR-0005).
- [ ] T053 `[adapter:daemon]` route framed requests through the shared
  application Facade (same use cases as the CLI).
- [ ] T054 `[root]` `main.rs` composition root (**Factory**/DI): build the one
  warm async-openai client, wire adapters into use cases, select CLI vs daemon.

## Verification

- [ ] T060 `[build]` `cargo build --release` GREEN; `cargo clippy --all-targets
  -- -D warnings`; `cargo fmt --all -- --check`; `cargo test`/`nextest`.
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
