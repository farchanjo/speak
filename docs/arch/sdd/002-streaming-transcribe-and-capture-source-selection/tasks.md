# Tasks: Streaming Transcribe And Capture Source Selection

Legend: `[x]` = implemented in the current tree, `[ ]` = pending.
Layer tags map to the Hexagonal plan; FR/ADR refs trace to spec + decisions.

## Phase 1 — streaming transcribe + source plumbing + BlackHole fallback ✅

- [x] T001 [domain] `CaptureSource` value object: `Input { device, channel }` /
      `Output { device, channel }` (+ `direction()`, `CaptureDirection::parse`).
      Unit tests. (FR-3 / ADR-0015)
- [x] T002 [ports] `AudioSource::capture_for(source: &CaptureSource, secs)`
      default method routing Input → `capture`, Output → actionable error; the
      device-only `capture` stays for the input path. (FR-3/FR-4)
- [x] T003 [adapters/coreaudio] No adapter change needed in Phase 1: the port
      default handles `Input` (existing path) and returns the BlackHole-fallback
      error for `Output` until the native tap lands (T011). (FR-5/FR-6/FR-9)
- [x] T004 [application] `StreamTranscribeUseCase`: capture-and-gate one chunk
      (shared `capture::capture_gated`, `CaptureSource`), drive the
      `RealtimeStream` transcript-only — surface `transcript`, ignore
      `audio`/`translation`, end on `done`, surface `error`. `FakeStream` tests.
      (FR-1/FR-2/ADR-0014)
- [x] T005 [application] Facade `stream_transcribe_capture`/`stream_transcribe_drive`;
      shared `capture` module; realtime + record capture-and-gate migrated to
      `CaptureSource` with no behavior change to the input arm. (FR-1/FR-4)
- [x] T006 [cli/args] `transcribe`: `-S/--stream`, `FILE` optional in stream
      mode; `-s/--source`, `-d`, `-I`, `-c/--chunk`, `-x/--no-vad`, `-F`. (FR-1/FR-7/FR-12)
- [x] T007 [cli/args] `realtime` + `record`: `-s/--source` (flag > config). (FR-3/FR-8/FR-12)
- [x] T008 [cli + main.rs] `transcribe` streaming branch; `SseRealtimeClient`
      built in the `Transcribe` dispatch arm when `--stream`; transcript lines;
      Ctrl-C stop. (FR-1/FR-11)
- [x] T009 [adapters/config] `[audio.capture]` (`source` + output device/channel),
      `SPEAK_AUDIO_CAPTURE_*` overrides, defaults, `config show` origin,
      `config init` template. (FR-10)
- [x] T010 [tests/docs] hermetic CLI/parse tests, config origin test; CLAUDE.md
      §4/§6 + acceptance-coverage trace; BlackHole fallback documented.
      `make gates` green. (FR-1..FR-12)

## Phase 2 — native macOS Core Audio output tap (on-device verification)

API **confirmed** present in the linked `objc2-core-audio` 0.3.2 (`AudioHardware`
feature); exact sequence, signatures, Cargo features, and friction points are in
`research.md`. Implementation is a write → `make build-dbg` → run-on-Mac → lldb
loop (CLAUDE.md §7), so it is done with the device in the loop, not headless.

- [ ] T011 [adapters/coreaudio/macos] `tap.rs::capture_output`: stereo global
      `CATapDescription` → `AudioHardwareCreateProcessTap` → private auto-start
      `AudioHardwareCreateAggregateDevice` embedding the tap → reuse
      `engine::capture(Some(agg_id), secs)` → RAII teardown. Override
      `CoreAudio::capture_for` `Output` arm. See `research.md`. (FR-5)
- [ ] T012 [adapters/coreaudio/macos] Permission + availability: detect
      macOS < 14.4 / `OSStatus` failures / denied TCC capture and return the
      actionable error naming the BlackHole fallback (FR-9).
- [ ] T013 [verify] On-device validation (audio playing, permission granted) of
      `transcribe --stream --source output`, `record --source output`,
      `realtime --source output`; lldb-read `tap_id`/`agg_id`/`OSStatus`/RMS.
      Bind the four pending scenarios in the acceptance trace.

## Dependencies

- Phase 1 ships and is committable without Phase 2 (BlackHole fallback covers
  output capture meanwhile).
- Phase 2 needs macOS 14.4+ hardware with audio-capture permission and the
  `objc2-core-audio` tapping symbols (or a raw FFI shim if absent in 0.3.2).
- Server: reuses the existing `POST /v1/realtime/translate` SSE route (ADR-0004);
  no server change required.
