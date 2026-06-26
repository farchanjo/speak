# Feature Specification: speak — speech client for the solaris server

Feature: 001-speak-cli-speech-client-for-solaris-server
Created: 2026-06-26
Status: draft

## Summary

`speak` is a single static Rust binary: a network client for the OpenAI-compatible
speech server running on `http://solaris:8800` (OmniVoice TTS + faster-whisper ASR
on an RTX 4090). It exposes Text-to-Speech, Speech-to-Text, and a realtime
translation pipeline, is trivially configurable, and works fully over the network.

## User Stories

- As a CLI user I want `speak say "texto"` to synthesize speech on the server and
  play it locally, so that I get high-quality TTS from one short command.
- As a CLI user I want `speak transcribe audio.mp3` to return a transcript, so that
  I can turn any audio file into text.
- As a CLI user I want `speak translate audio.mp3` to return the English translation
  of foreign-language audio, so that I can understand it.
- As a CLI user I want `speak realtime` to capture my microphone, transcribe it live,
  translate it, and speak the translation, so that I get hands-free live translation.
- As a user I want configuration via a TOML file, environment variables, and flags
  with clear precedence, so that I can set the server host/key once and override per call.
- As an integrator I want the client to speak both the OpenAI audio API and the
  server's native `/tts`, so that I can target whichever endpoint I prefer.

## Functional Requirements

1. **TTS** — `speak say|tts <text>` POSTs to `/v1/audio/speech` (OpenAI schema:
   `model,input,voice,response_format,speed,language`) OR native `/tts` when
   `--native`. Default `language=pt-BR`. Plays audio locally unless `-o FILE`/`--no-play`.
2. **STT** — `speak transcribe <file>` POSTs multipart to `/v1/audio/transcriptions`.
   Supports `--format json|text|srt|vtt|verbose_json` and `--language`.
3. **Translate** — `speak translate <file>` POSTs to `/v1/audio/translations` (to English).
4. **Realtime** — `speak realtime [--from LANG] [--to LANG] [--speak]` loops:
   capture mic (chunked + silence split) → ASR → translation → print, and optionally
   TTS-play the translation. Streams continuously until Ctrl-C.
5. **Config precedence** — CLI flags > env (`SPEAK_HOST`,`SPEAK_API_KEY`,`SPEAK_LANG`,
   `SPEAK_VOICE`,`SPEAK_FORMAT`) > TOML (`~/.config/speak/config.toml`) > built-in defaults
   (host `http://solaris:8800`, lang `pt-BR`, format `mp3`).
6. **Health/config** — `speak health` checks `/health`; `speak config init|path|show`
   manages the TOML.
7. **Compatibility** — Works against any OpenAI-audio-compatible server via `--host`.
8. **Output** — `--quiet`, `--json` where applicable; non-zero exit on HTTP/transport error.

## Non-Functional Requirements

- Async I/O (tokio + reqwest), streaming bodies where the server supports it.
- **All media is in-process — no exec.** Server audio is decoded and resampled
  with linked `libav*` (ffmpeg-the-third 5.0.0+ffmpeg-8.1, libavcodec 62) via a
  custom in-memory AVIO callback. Playback and microphone capture use the
  **native macOS CoreAudio mixer** (`AVAudioEngine`: `AVAudioPlayerNode` ->
  `mainMixerNode` -> `outputNode`; `inputNode` tap for capture) through the
  `objc2-avf-audio` bindings. No `ffmpeg`/`ffplay`/`afplay`/`cpal`.
- Native device I/O is macOS-only today; on other platforms playback/capture
  return a clear error while file-oriented commands keep working.
- Single self-contained binary; config optional (sane defaults work out of the box).
- Latency: a `say` round trip should be dominated by server inference, not the client.

## Acceptance Scenarios

Given a reachable server at `$SPEAK_HOST`
When I run `speak say "olá"`
Then the server synthesizes pt-BR audio and it plays locally with exit code 0.

Given an audio file `a.mp3`
When I run `speak transcribe a.mp3 --format text`
Then stdout is the transcript text and exit code is 0.

Given foreign-language audio `f.mp3`
When I run `speak translate f.mp3`
Then stdout is the English translation.

Given a microphone and `--speak`
When I run `speak realtime --from en --to pt-BR --speak`
Then each utterance is transcribed, translated to pt-BR, printed, and spoken, until Ctrl-C.

Given no config file and no flags
When I run `speak say "oi"`
Then it uses the default host `http://solaris:8800` and succeeds.

## Out of Scope

- Hosting/serving models (the server already does that).
- GUI; bundling ffmpeg/afplay (assumed present or installed by the user).
- Arbitrary text machine-translation without an LLM endpoint (realtime uses Whisper
  translate→English by default; arbitrary target requires an optional chat endpoint).
