---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Streaming transcribe over the realtime SSE endpoint (transcript-only)

## Context and Problem Statement

`speak transcribe` is one-shot: it reads a complete audio file, POSTs it to
`/v1/audio/transcriptions`, and prints the full transcript when the server
finishes (feature 001, FR-6). Users want a **live** transcript — speak (or play)
into the tool and see the text appear incrementally, hands-free, without
re-voicing.

The server exposes exactly one streaming route, `POST /v1/realtime/translate`,
which runs ASR → (optional MT) → TTS and emits Server-Sent Events
(`transcript | translation | audio | done | error`). With `translate=false` it
runs ASR-only and streams the source-language `transcript` frames (and,
currently, re-voiced `audio` frames). There is **no** dedicated streaming-ASR
endpoint (no WebSocket, no chunked partials on `/v1/audio/transcriptions`).

How should `speak` deliver streaming transcription without a new server route?

## Decision Drivers

- One prebuilt binary working against the existing server; no new endpoint.
- Reuse the proven realtime capture + SSE consumer (ADR-0004) rather than
  building a parallel pipeline.
- Keep `transcribe` text-only: no re-voicing, no playback, no output devices.
- Honor the Presenter/`tracing` discipline (ADR-0009) and the bounded reconnect
  retry policy (ADR-0004).

## Considered Options

- **Option A** — `transcribe --stream` reuses the SSE adapter and the
  `RealtimeStream` port: post each captured chunk with `translate=false`,
  surface only `transcript` frames, ignore `audio`/`translation` frames, never
  play. A small transcript-only application use case drives the stream so the
  full re-voicing `RealtimeUseCase` (which auto-plays `audio` frames) is not
  reused verbatim.
- **Option B** — Extend `realtime` with a `--no-speak` flag that suppresses
  playback and prints transcripts, folding streaming transcribe into the
  realtime command.
- **Option C** — Wait for / require a dedicated server streaming-ASR endpoint
  (WebSocket or chunked) and build a new adapter.

## Decision Outcome

Chosen option: **Option A**.

- `transcribe` gains a `--stream` flag. In file mode (no `--stream`) it is
  unchanged. In stream mode it captures live audio (the shared `CaptureSource`
  Strategy of ADR-0015), encodes each gated chunk to WAV, and posts it to
  `POST /v1/realtime/translate` with `translate=false` using the existing
  `SseRealtimeClient` / `RealtimeRequest` and the bounded `ReconnectingStream`.
- A dedicated **transcript-only drive** consumes the `RealtimeStream`: it maps
  `transcript` frames to a Presenter line, **ignores `audio` frames without
  decoding or playing them**, ignores `translation` frames, ends on `done`, and
  logs/surfaces `error`. This is why the existing `RealtimeUseCase` is not
  reused as-is: its `pump_frame` decodes and plays `audio` frames, which
  streaming transcribe must not do. The new use case depends only on the
  `RealtimeStream` port (plus the shared capture-and-gate step) — no
  `Synthesizer`, `Translator`, or `AudioSink` — keeping dependencies pointed
  inward and the surface minimal (ADR-0003).
- The request still carries the configured output voice and `format` so the
  server's pipeline accepts the chunk exactly as the proven
  `realtime --no-translate` path does; the returned `audio` frames are simply
  discarded client-side. This trades some wasted server-side TTS work for
  reusing one validated contract. A server-side `tts=false` field that skips
  re-voicing when only the transcript is wanted is recorded as **future server
  work**; it does not block this client feature and would be a transparent
  optimization (the client already ignores `audio` frames).
- `--stream` is mutually informative with the source selector of ADR-0015:
  `--source input` (default) streams the microphone; `--source output` streams
  the system/sound-card output.

This rejects Option B (overloading `realtime` with a non-speaking mode muddies
its re-voicing Strategy and its output-device flags, none of which apply to
transcription) and Option C (no server endpoint exists; the SSE reuse ships
today).

### Consequences

- Good: streaming transcription ships against the current server with no
  protocol change; it reuses the SSE adapter, the reconnect decorator, and the
  capture-and-gate step already covered by tests.
- Good: `transcribe` stays text-only and pipeable; the transcript-only use case
  is tiny and unit-testable with the existing `FakeStream`.
- Bad: with `translate=false` the server still synthesizes re-voiced `audio`
  frames that the client throws away — wasted upstream work until a server-side
  `tts=false` toggle exists.
- Bad: transcript granularity is per-chunk (the server streams finalized
  per-chunk text, not revisable word-level partials); chunk length trades
  latency against transcript coherence.
- Neutral: this ADR amends ADR-0004 (the SSE realtime decision) by adding a
  second consumer of the same endpoint with a different frame-handling policy.
