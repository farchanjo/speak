# Implementation Plan: speak

## Overview

Build `speak`, a single Rust binary that is a network client for the OpenAI-compatible
speech server at `http://solaris:8800` (OmniVoice TTS + faster-whisper ASR). Goal: TTS,
STT, translation, and a realtime translation pipeline, with trivial configuration.

## Technical Approach

- Rust 2021, async via **tokio**; HTTP via **reqwest** (json + multipart + streaming).
- **clap** (derive) CLI with `ValueEnum` choices, env-aware global flags, and a
  `completions` subcommand (`clap_complete`); **serde + toml** config;
  **anyhow** errors. Config dir resolved manually to honour `XDG_CONFIG_HOME`.
- **In-process media, no exec.** `ffmpeg-the-third` (libav FFI) for codecs only;
  native macOS CoreAudio (`objc2-avf-audio` `AVAudioEngine`) for device I/O.

### Modules
- `config.rs` — `Config`/`FileConfig` + precedence: flags > env (`SPEAK_*`) >
  `~/.config/speak/config.toml` > defaults.
- `client.rs` — `SpeechClient`: `health`, `speak` (`/v1/audio/speech` + native
  `/tts`), `transcribe`, `translate`, optional `chat_translate`.
- `codec.rs` — libav codec layer: custom in-memory AVIO decode -> PCM,
  libswresample resample (48 kHz stereo f32 for playback, 16 kHz mono s16 for
  ASR), in-memory WAV mux, RMS silence gate.
- `audio_macos.rs` — native CoreAudio: `play` (AVAudioPlayerNode ->
  `mainMixerNode` -> output) and `capture_chunk` (inputNode tap). Gated
  `#[cfg(target_os = "macos")]`; `audio_stub.rs` errors elsewhere.
- `main.rs` — commands `say|tts`, `transcribe`, `translate`, `realtime`,
  `health`, `config`, `completions`.

### Realtime pipeline
mic (native CoreAudio tap) -> libav resample to 16 kHz mono + in-memory WAV
(silence-skipped via RMS gate) -> ASR(`--from`) -> target `--to`: `en` =>
Whisper translate; else optional chat MT, else source transcript -> print +
optional `--speak` TTS playback. Loops until Ctrl-C.

## Companion Artifacts

- `contracts/` — the server's OpenAI audio endpoints (`/v1/audio/speech|transcriptions|translations`)
  plus native `/tts` and `/health`.
- `quickstart.md` — `cargo build --release`; `speak health`; `speak say "oi"`.
