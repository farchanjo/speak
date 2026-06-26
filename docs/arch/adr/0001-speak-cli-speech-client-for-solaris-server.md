---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# speak — native-media speech client for the solaris server

## Context and Problem Statement

`speak` is a single Rust binary that talks to an OpenAI-compatible speech
server (`http://solaris:8800`: OmniVoice TTS + faster-whisper ASR). It must
synthesize speech and play it locally, transcribe/translate audio files, and
run a live microphone-translation loop. The hard constraint: **all media
handling must be in-process** — decode, playback and capture must use linked
libraries via FFI, never a child process (`ffmpeg`, `ffplay`, `afplay`).

## Decision Drivers

- No process exec for any media operation (decode, play, capture).
- Decode bleeding-edge codecs (ffmpeg 8.1 / libavcodec 62) from server bytes.
- Native OS audio path with a real mixer; everything processed in-binary.
- Single self-contained binary; sane defaults; trivially configurable.
- Idiomatic async Rust (tokio + reqwest), clippy-clean.

## Considered Options

- Option A — ffmpeg-the-third (libav FFI) for codecs + native CoreAudio
  (AVAudioEngine) for device I/O.
- Option B — ffmpeg FFI for codecs + `cpal` (pure-Rust) for output.
- Option C — Shell out to `ffmpeg`/`afplay` for media.

## Decision Outcome

Chosen option: "Option A", because it is the only option that both honours the
no-exec rule and routes device I/O through the native OS mixer.

- Codecs (decode + resample): `ffmpeg-the-third 5.0.0+ffmpeg-8.1` links
  libavcodec/format/util/swresample. The build script auto-detects the
  installed ffmpeg and enables the `ffmpeg_8_1` cfg, binding libavcodec 62.
  Server audio bytes are decoded through a custom in-memory AVIO read callback
  (`avio_alloc_context` + `AVFMT_FLAG_CUSTOM_IO`) — no temp files, no exec —
  then resampled with libswresample to canonical 48 kHz stereo f32 for
  playback, or 16 kHz mono s16 for ASR upload.
- Device I/O + mixing (native): macOS CoreAudio via `objc2-avf-audio`
  (`AVAudioEngine`). Playback graph: `AVAudioPlayerNode` -> `mainMixerNode`
  (the native OS mixer) -> `outputNode`; decoded PCM is scheduled as
  `AVAudioPCMBuffer`s. Capture: `AVAudioEngine.inputNode` with
  `installTapOnBus`, resampled by libav to 16 kHz mono, muxed to an in-memory
  WAV (hand-written RIFF/WAVE header) and POSTed to Whisper.
- Recording output encode (`speak record --format wav|flac`, FR-9): captured
  PCM is written either as the hand-muxed in-memory WAV (no encoder) or, for
  `--format flac`, encoded with the libavcodec FLAC encoder through a custom
  in-memory AVIO **write** callback (`avio_alloc_context` write side, mirror of
  the decode read callback) — still no temp files and no exec. The libav
  adapter therefore covers both the decode/resample direction and this
  encode/mux direction.
- Non-macOS: device I/O returns a clear error; file commands still work.

### Consequences

- Good: zero media subprocesses; native CoreAudio mixer; latest ffmpeg codecs;
  compact binary linking Homebrew dylibs.
- Good: one internal `unsafe` surface (libav FFI + AVFAudio), each block
  SAFETY-commented; Objective-C exceptions are caught via
  `objc2::exception::catch` so the engine can never abort the process.
- Bad: native playback/capture is macOS-only today (Linux/Windows are future
  work); the build requires `PKG_CONFIG_PATH` + `LIBCLANG_PATH` for the FFI
  bindgen step.
