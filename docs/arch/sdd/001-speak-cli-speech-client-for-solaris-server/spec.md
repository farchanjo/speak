# Feature Specification: Speak Cli Speech Client For Solaris Server

Feature: 001-speak-cli-speech-client-for-solaris-server
Created: 2026-06-26
Status: draft

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
`adapters` for OpenAI HTTP, CoreAudio, libav, config, daemon, and SSE.

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
   `--output-device` is repeatable (single device or fan-out, FR-11).
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
   `/v1/audio/translations` (audio to English text).
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
   otherwise the client falls back to the chunked ASR->MT->TTS loop. Loops until
   Ctrl-C.
9. **Record** — `speak record` captures the microphone to a file
   (`--output`, `--device`, `--format wav|flac`, `--duration`, `--sample-rate`,
   `--channels`).
10. **Devices** — `speak devices [--json]` lists input and output audio devices
    (CoreAudio enumeration), including the `AudioDeviceID`s used by
    `--output-device` and `[audio.*].device`.
11. **Multi-output routing** — `--output-device` is repeatable on `say` and
    `realtime`; one device pins one engine, many devices fan one decode out to N
    engines (or an aggregate device), fully digital, no exec (ADR-0007).
12. **Daemon** — `speak daemon [--foreground|stop|status]` runs a process that
    holds one warm pooled async-openai client and listens on a Unix socket
    (`[daemon].socket`, default `~/.speak/speak.sock`); CLI commands forward to
    it (length-prefixed framing, SSE frames streamed through) with transparent
    one-shot fallback when no daemon is present (ADR-0005).
13. **Config precedence and catalog** — `CLI flag > ENV (SPEAK_*) >
    ~/.speak/config.toml > code default` (ADR-0006). The file migrates from
    `~/.config/speak` if present. `config init` writes a fully-commented TOML;
    `config show` prints every effective value AND its origin
    (`flag|env|toml|default`); `config path` prints the resolved path. The full
    catalog of sections/keys is defined in ADR-0006 and the CUE schema.
14. **Health / models** — `speak health` checks `GET /health`; the client also
    reads `GET /v1/models`. `speak check` reports OS/arch, CPU cores, libav
    hwdevice types, AudioToolbox decoders, and the active acceleration policy
    (`SPEAK_HWACCEL=auto|off|<decoder>`); rotating logs go under `~/.speak/logs`
    (`SPEAK_LOG`, `SPEAK_LOG_DIR`; `SPEAK_LOG=off` disables) — ADR-0002.
15. **Completions** — `speak completions <shell>` emits a shell completion script.
16. **Compatibility & output** — works against any OpenAI-audio-compatible server
    via `--host`; honours `--quiet` and `--json` where applicable; returns a
    non-zero exit on HTTP/transport error.

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

## Out of Scope

- Hosting/serving models (the server already does that).
- A GUI; bundling or shelling out to ffmpeg/afplay/ffplay (all media is linked
  in-process).
- Arbitrary text machine-translation without an LLM endpoint (realtime uses
  Whisper translate -> English by default; an arbitrary target requires
  `translate_url`/`translate_model`).
- Windows/Linux native audio device I/O (future work); file commands still run.
