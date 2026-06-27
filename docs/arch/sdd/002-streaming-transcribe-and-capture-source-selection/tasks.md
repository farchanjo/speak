# Tasks: Streaming Transcribe And Capture Source Selection

Legend: `[x]` = implemented in the current tree, `[ ]` = pending.
Layer tags map to the Hexagonal plan; FR/ADR refs trace to spec + decisions.

## Phase 1 — streaming transcribe + source plumbing + BlackHole fallback

- [ ] T001 [domain] `CaptureSource` value object: `Input { device, channel }` /
      `Output { device, channel }` (+ direction accessor, validation). Unit
      tests. (FR-3 / ADR-0015)
- [ ] T002 [ports] `AudioSource::capture(source: &CaptureSource, secs)`; keep the
      device-only signature as a thin shim or migrate call sites. (FR-3/FR-4)
- [ ] T003 [adapters/coreaudio] Route `capture` by source: `Input` = existing
      path; `Output` = native tap entry point, returning a clear
      "output capture needs macOS 14.4+ native tap (Phase 2) — route via a
      virtual-loopback input device meanwhile" error until T010. Non-macOS stub
      unchanged. (FR-5/FR-6/FR-9)
- [ ] T004 [application] `StreamTranscribeUseCase`: capture-and-gate one chunk
      (shared step, `CaptureSource`), drive the `RealtimeStream` transcript-only
      — surface `transcript`, ignore `audio`/`translation`, end on `done`,
      surface `error`. Unit tests with `FakeStream`. (FR-1/FR-2/ADR-0014)
- [ ] T005 [application] Facade method for streaming transcribe + capture step
      reuse; refactor the realtime capture-and-gate to take `CaptureSource`
      without behavior change to the input arm. (FR-1/FR-4)
- [ ] T006 [cli/args] `transcribe`: `--stream` (short), make `FILE` optional in
      stream mode; add `--source`, `-d/--device`, `-I/--input-channel`,
      `-c/--chunk`, `-x/--no-vad`, `-F/--vad-floor`. (FR-1/FR-7/FR-12)
- [ ] T007 [cli/args] `realtime` + `record`: add `--source` (and the output
      device/channel mapping). (FR-3/FR-8/FR-12)
- [ ] T008 [cli + main.rs] `transcribe` handler streaming branch; construct
      `SseRealtimeClient` in the `Transcribe` dispatch arm when `--stream`.
      Present transcript lines; Ctrl-C stop. (FR-1/FR-11)
- [ ] T009 [adapters/config] `[audio.capture].source` (+ output device/channel),
      `SPEAK_*` overrides, defaults, `config show` origin, `config init` docs.
      (FR-10)
- [ ] T010 [tests/docs] hermetic CLI tests (`tests/cli.rs`): `--stream` flag
      parsing, `--source` parsing/precedence, file-mode unchanged; update
      CLAUDE.md §4/§6 + acceptance-coverage trace; document BlackHole fallback.
      `make gates` green. (FR-1..FR-12)

## Phase 2 — native macOS Core Audio output tap (on-device verification)

- [ ] T011 [adapters/coreaudio/macos] Implement `Output` capture: build
      `CATapDescription` (system mix default, or selected output device),
      `AudioHardwareCreateProcessTap`, embed in a private
      `AudioHardwareCreateAggregateDevice`, capture the aggregate stream, then
      destroy the aggregate device + tap. Reuse the single-channel pick
      (ADR-0013) + WAV/resample. Replaces the T003 placeholder. (FR-5)
- [ ] T012 [adapters/coreaudio/macos] Permission + availability handling: detect
      macOS < 14.4 / missing tapping symbols / denied capture and return the
      actionable error (FR-9); add the capture-usage description if required.
- [ ] T013 [verify] On-device validation (real output playing, permission
      granted) of `transcribe --stream --source output`,
      `record --source output`, `realtime --source output`; debugger-grounded
      checks of the tap/aggregate IDs. Update acceptance-coverage trace.

## Dependencies

- Phase 1 ships and is committable without Phase 2 (BlackHole fallback covers
  output capture meanwhile).
- Phase 2 needs macOS 14.4+ hardware with audio-capture permission and the
  `objc2-core-audio` tapping symbols (or a raw FFI shim if absent in 0.3.2).
- Server: reuses the existing `POST /v1/realtime/translate` SSE route (ADR-0004);
  no server change required.
