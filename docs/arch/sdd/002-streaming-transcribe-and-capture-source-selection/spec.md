# Feature Specification: Streaming Transcribe And Capture Source Selection

Feature: 002-streaming-transcribe-and-capture-source-selection
Created: 2026-06-27
Status: draft

## Summary

Two related capabilities layered onto the existing capture pipeline:

1. **Streaming transcribe** — `speak transcribe --stream` captures live audio and
   emits the source-language transcript incrementally (one line per server frame)
   until `Ctrl-C`, with **no re-voicing and no playback**. It is the ASR-only
   sibling of `realtime`: it reuses the server's `POST /v1/realtime/translate`
   Server-Sent Events endpoint with `translate=false` and surfaces only the
   `transcript` frames (ADR-0014).

2. **Capture source selection** — a `--source input|output` selector (and
   `[audio.capture].source`) shared by `transcribe --stream`, `realtime`, and
   `record`. `input` (default) captures an audio input device/channel as today
   (mic / line-in, ADR-0011 / ADR-0013). `output` captures what the host is
   **playing** — the sound-card / system output — directly on the machine with
   **no hardware loopback**, via a native macOS Core Audio process/output tap
   (macOS 14.4+), with a routed virtual-loopback device (e.g. BlackHole) as a
   documented fallback (ADR-0015).

Both stay inside the project's constraints: Hexagonal + DDD + GoF (ADR-0003),
zero media process-exec (native CoreAudio tap, no shelling out), the layered
config catalog (ADR-0006), and the Presenter output port + `tracing`
diagnostics (ADR-0009). macOS arm64 is the native target; other platforms get a
clear-error stub.

## User Stories

- As a CLI user I want `speak transcribe --stream` to print a live transcript of
  what I am saying into the microphone, so that I get hands-free dictation
  without re-voicing or waiting for a file.
- As a CLI user I want `speak transcribe --stream --source output` to transcribe
  what my computer is playing (a call, a video, an audio stream), so that I can
  caption system audio when my audio interface has no hardware loopback.
- As a CLI user I want `--source output` to work directly on the PC without
  installing a driver where the OS supports it, and to fall back to a routed
  virtual-loopback device (BlackHole) where it does not, so that I am not blocked
  by my hardware.
- As a CLI user I want the same `--source input|output` selection on
  `speak realtime` and `speak record`, so that live translation and recording can
  also target system output, not just the microphone.
- As a CLI user I want a clear error (not a crash) when the OS denies audio
  capture or the native tap is unavailable, telling me what to do next.

## Functional Requirements

1. **FR-1 — Streaming transcribe.** `speak transcribe --stream` captures live
   audio in chunks and emits the source-language transcript incrementally (one
   Presenter line per surfaced frame) until `Ctrl-C`. It performs **no
   translation, no re-voicing, and no audio playback**. Without `--stream`,
   `transcribe` keeps its current file-based one-shot behavior (FR-6 of feature
   001); the positional `FILE` is required in file mode and ignored/omitted in
   `--stream` mode.
2. **FR-2 — Reuse the realtime SSE endpoint.** Streaming transcribe posts each
   captured chunk to `POST /v1/realtime/translate` with `translate=false` and
   consumes the SSE frames via the existing `RealtimeStream` port. Only
   `transcript` frames are surfaced to the user; `translation` frames (absent
   when `translate=false`) and `audio` frames (server re-voicing) are **ignored
   without playback**; `done` ends the chunk and `error` is logged and surfaced.
   No new server endpoint is required (ADR-0014).
3. **FR-3 — Capture source selector.** A `--source <input|output>` flag (default
   `input`) and an `[audio.capture].source` config key select the capture source
   for `transcribe --stream`, `realtime`, and `record`. The selection is a
   **Strategy** chosen at the composition root; the rest of each pipeline is
   source-agnostic.
4. **FR-4 — Input source.** `--source input` captures an audio input device
   (microphone / line-in) with optional device selection (`-d/--device`,
   ADR-0011) and a single 0-based capture channel (`-I/--input-channel`,
   ADR-0013). This is the existing capture path, unchanged in behavior.
5. **FR-5 — Native output capture.** `--source output` captures the host's
   playback ("what the PC is playing") **directly on the machine with no
   hardware loopback**, via a native macOS Core Audio process/output tap
   (macOS 14.4+: `AudioHardwareCreateProcessTap` + a private aggregate device).
   It captures the system output mix by default and may be pinned to a specific
   output device (`-d/--device`) and a single capture channel
   (`-I/--input-channel`). It is driver-free.
6. **FR-6 — Virtual-loopback fallback.** Where the native tap is unavailable
   (macOS < 14.4, denied permission, or user preference), the user routes the
   output to a virtual-loopback device (e.g. BlackHole / an aggregate device)
   that presents as an **input** device, and captures it with the input source
   path (`--source input -d <loopback-id> [-I <ch>]`). This needs no special
   code — a routed loopback is just another input device — and is covered by
   documentation (ADR-0015).
7. **FR-7 — Streaming controls.** Streaming transcribe honors the silence/VAD
   gate (`-x/--no-vad`, `-F/--vad-floor <dBFS>`), the chunk length
   (`-c/--chunk <secs>`), and the source-language hint (`-l/--language <LANG>`).
   Defaults come from `[audio.input]` / `[asr]` per the config catalog.
8. **FR-8 — Record from output.** `speak record --source output -o file.wav`
   captures the system output to a WAV/FLAC file with the same encoding path as
   microphone recording today.
9. **FR-9 — Permission and availability errors.** When the OS denies audio
   capture, or the native output tap cannot be created (unsupported OS, missing
   entitlement/permission), the command fails with a clear, actionable error
   (what was denied, how to grant it, and the BlackHole fallback) — never a
   panic or a silent empty transcript.
10. **FR-10 — Config catalog.** Every new tunable (`[audio.capture].source`,
    output device, output channel) has a `SPEAK_*` env override and a code
    default under the precedence `flag > env > toml > default`; `config init`
    documents them and `config show` reports each value's origin (ADR-0006).
11. **FR-11 — Output discipline.** Streaming transcribe emits transcripts through
    the Presenter port (one `line` per surfaced transcript, pipeable under
    `--quiet`, structured under `--json`); all diagnostics go to `tracing`, with
    no raw `println!` in the layers (ADR-0009).
12. **FR-12 — Exhaustive short flags.** Every new option has a short flag
    (ADR-0012); `--source` and `--stream` get short forms that do not collide
    with the per-subcommand flag set.

## Capture Source Model

`CaptureSource` is a pure domain **value object** (Strategy selector) with two
variants, each carrying an optional device id and an optional 0-based capture
channel:

- `Input { device: Option<AudioDeviceId>, channel: Option<u16> }` — capture an
  input device; `device = None` uses the system default input.
- `Output { device: Option<AudioDeviceId>, channel: Option<u16> }` — capture an
  output device's playback via the native tap; `device = None` taps the system
  output mix.

The `AudioSource` port gains a source-aware capture so the CoreAudio adapter can
implement the output tap behind the same boundary (no `objc2` type crosses it).
The application capture-and-gate step takes a `CaptureSource` instead of a bare
device id; the `input` arm is behaviorally identical to today.

## Non-Functional Requirements

- **Zero media process-exec.** Output capture is a native Core Audio tap; the
  binary never shells out to `ffmpeg`, `afplay`, BlackHole CLIs, or any media
  process (ADR-0001).
- **Platform.** macOS arm64 native; the native tap lives behind the existing
  `cfg(target_os = "macos")` gate, with a clear-error stub elsewhere.
- **Latency parity.** Streaming transcribe per-chunk latency is comparable to
  `realtime --no-translate` (same capture + SSE round trip), minus TTS.
- **Resilience.** The SSE stream reconnects under the existing bounded
  `[retry]` policy (ADR-0004); a dropped chunk does not end the session.
- **Daemon.** Streaming transcribe runs in the CLI driving-adapter loop like
  `realtime`; it does not change the daemon protocol.

## Out of Scope

- A dedicated server-side streaming-ASR endpoint (WebSocket or chunked partial
  hypotheses). This feature reuses the existing SSE `translate=false` path.
- Revisable interim/partial word hypotheses. The server streams per-chunk
  finalized transcript text; the Presenter appends one line per frame.
- System-audio capture on Windows/Linux.
- A server-side `tts=false` flag to skip re-voicing work when only the transcript
  is wanted — noted as a future server optimization in ADR-0014; until then the
  client ignores the returned `audio` frames.

## Acceptance Scenarios

Given a reachable speech server with the realtime SSE endpoint
When  I run `speak transcribe --stream`
Then  the microphone is captured live
And   each server `transcript` frame is printed as a line
And   no audio is played
And   `Ctrl-C` stops the loop with exit code 0

Given the host is playing audio and the OS supports the native tap
When  I run `speak transcribe --stream --source output`
Then  the system output is captured via the native Core Audio tap
And   the transcript of the playing audio is printed incrementally

Given macOS audio-capture permission is denied for output capture
When  I run `speak transcribe --stream --source output`
Then  the command fails with a clear, actionable error
And   the error names the BlackHole fallback

Given a BlackHole device is routed from the system output
When  I run `speak transcribe --stream --source input -d <blackhole-id>`
Then  the routed output is captured through the input source path
And   the transcript is printed incrementally

Given the host is playing audio
When  I run `speak record --source output -o sys.wav`
Then  a WAV file of the system output is written
And   the exit code is 0

Given a non-default `[audio.capture].source = "output"` in config
When  I run `speak config show`
Then  the value `output` is reported with origin `toml`
