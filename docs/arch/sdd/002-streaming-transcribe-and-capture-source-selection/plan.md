# Implementation Plan: Streaming Transcribe And Capture Source Selection

## Overview

Deliver two capabilities on top of the existing capture pipeline (feature 001):
live **streaming transcribe** (`transcribe --stream`, transcript-only over the
realtime SSE endpoint, ADR-0014) and a shared **capture source selector**
(`--source input|output`) that adds native macOS system/output capture with a
BlackHole fallback (ADR-0015), across `transcribe --stream`, `realtime`, and
`record`. Dependencies point inward; framework crates stay in `adapters`/`cli`.

## Technical Approach

- **Streaming transcribe** reuses `SseRealtimeClient` + `RealtimeRequest` +
  `ReconnectingStream` (ADR-0004) with `translate=false`. A new
  `StreamTranscribeUseCase` drives the `RealtimeStream` port transcript-only: it
  surfaces `transcript` frames as Presenter lines, **ignores `audio` and
  `translation` frames without playback**, ends on `done`, surfaces `error`. It
  is not the existing `RealtimeUseCase` (which auto-plays audio frames).
- **Capture source** is a pure `CaptureSource` domain value object
  (`Input{device,channel}` / `Output{device,channel}`). The `AudioSource` port
  gains `capture(source, secs)`; the application capture-and-gate step takes a
  `CaptureSource`. The CoreAudio adapter implements `Input` as today and
  `Output` as a native Core Audio HAL tap (macOS 14.4+) — `CATapDescription` +
  `AudioHardwareCreateProcessTap` + a private aggregate device — reusing the
  single-channel pick (ADR-0013) and the WAV/resample path. Pre-14.4 / non-macOS
  / denied permission returns a clear error naming the BlackHole fallback.
- **CLI**: `--source` + `--stream` (short flags, ADR-0012) on the three capture
  commands; the active source's device is `-d/--device`, its channel
  `-I/--input-channel`. `transcribe` makes the positional `FILE` optional in
  stream mode.
- **Config**: `[audio.capture].source` (+ output device/channel) with `SPEAK_*`
  overrides, code defaults, and reported origin (ADR-0006); `config init`
  documents them.
- **Output discipline**: transcripts via the Presenter `line`; diagnostics via
  `tracing` (ADR-0009). Zero media process-exec.

## Hexagonal module plan

```
domain/      CaptureSource value object (Input/Output + device + channel)
ports/       AudioSource::capture(source, secs); (RealtimeStream reused)
application/ StreamTranscribeUseCase (transcript-only RealtimeStream drive);
             capture-and-gate step takes CaptureSource; Facade method
adapters/    coreaudio: Output tap backend (macOS) + stub elsewhere;
             config: [audio.capture]; (sse/openai reused unchanged)
cli/         transcribe --stream + --source; realtime/record --source;
             main.rs dispatch wires SseRealtimeClient for transcribe --stream
```

## Phasing

- **Phase 1 (no new FFI, fully hermetic-testable):** `CaptureSource` domain +
  `AudioSource::capture(source,…)` with the `Output` arm returning a clear
  "not yet implemented, use BlackHole" error on macOS; `StreamTranscribeUseCase`
  + Facade + dispatch; `--stream`/`--source` flags on the three commands;
  `[audio.capture]` config; BlackHole fallback works immediately (a routed
  loopback is an input device). Tests + docs. `make gates` green. Commit.
- **Phase 2 (native FFI, on-device verification):** implement the CoreAudio
  `Output` tap (`CATapDescription` + `AudioHardwareCreateProcessTap` + aggregate
  device + capture + teardown), channel pick, permission/availability errors.
  Verified on real hardware with capture permission (debugger-grounded, not
  assumed from headers). Replaces the Phase-1 placeholder error.

## Companion Artifacts

- CUE schema: `docs/arch/schemas/streaming-transcribe-and-capture-source-selection.cue`
  (`#CaptureSource`, `#StreamingTranscribe`).
- Gherkin: `docs/arch/specs/features/streaming-transcribe-and-capture-source-selection.feature`.
- ADRs: `0014` (streaming transcribe over SSE), `0015` (capture source + native tap).
