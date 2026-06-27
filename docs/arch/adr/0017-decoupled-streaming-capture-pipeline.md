---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Decoupled streaming-capture pipeline (continuous producer + bounded queue)

## Context and Problem Statement

Streaming transcribe (`transcribe --stream`, ADR-0014) and `realtime` (ADR-0004)
drop words: spoken audio goes missing between chunks. The loop is **serial** —
`run_stream`/`realtime` capture exactly `chunk_secs` of audio, then POST the
chunk and **block** on the SSE round trip before capturing again:

```
[capture 0–5s][tear down][POST + SSE 5–7s  ← GAP, nothing captured][capture 7–12s]…
                          └ words spoken here are LOST ┘
```

Three losses, confirmed in the code:
1. **Capture↔process are serialized** — while a chunk is POSTed and the SSE
   stream is drained to `done`, capture is stopped. The `done` frame only
   arrives after the server's ASR **and** TTS re-voicing (the server still
   synthesizes even for `translate=false`, and we wait for it), so the gap is
   large.
2. **The output tap is rebuilt per chunk** — `capture_output` creates and
   destroys the whole tap + aggregate + IO proc every call (ADR-0015),
   widening the gap and adding overhead.
3. **Hard chunk boundaries** cut a word straddling the 5 s edge (secondary).

## Decision Drivers

- Never drop captured audio in the normal case (the user's complaint).
- Decouple capture from processing so a slow server/network never stalls
  capture — "queue to capture, process elsewhere, no task contention."
- Bound memory under a sustained-slow consumer.
- Keep transcripts in order; keep the hexagonal boundaries (no `tokio` type in
  the ports).

## Considered Options

- **Option A** — Keep the serial loop, only double-buffer (capture N+1 while
  POSTing N). Removes the *processing* gap but not the per-chunk **tap teardown**
  gap, so words still drop at boundaries.
- **Option B** — **Continuous producer + bounded queue + sequential consumer.**
  One native capture (tap/engine) runs for the whole session, filling a capped
  ring; a producer thread slices it into chunks onto a bounded channel; a single
  ordered consumer drains the channel and POSTs each chunk.
- **Option C** — Push raw frames to the server over a WebSocket and let it
  chunk. No such server endpoint exists (ADR-0014); out of scope.

## Decision Outcome

Chosen option: **Option B**.

- **Producer (continuous).** The CoreAudio adapter starts the native capture
  **once** per session — the output tap's `AudioDeviceIOProc` (ADR-0015) or the
  `AVAudioEngine` input tap — appending interleaved float frames to a shared
  **capped ring** (default ≈ 60 s; the RT callback drops the oldest frames when
  full). A dedicated producer thread owns the native capture, drains exactly
  `chunk_secs` of frames at a time into a `PcmBuffer`, and `blocking_send`s it to
  a bounded `tokio::sync::mpsc`. Dropping the receiver closes the channel, which
  the thread observes and tears the native capture down (RAII). The native
  capture never stops between chunks → **no gap**.
- **Capture stream.** The CoreAudio adapter exposes a `capture_stream(source,
  chunk_secs, cap_secs)` constructor returning a concrete `NativeCaptureStream`
  with `async fn recv(&mut self) -> Option<PcmBuffer>` (wrapping the
  `mpsc::Receiver`; `tokio` stays inside the adapter, like the `sse` adapter).
  The continuous capture is wired by the **driving adapter** (the streaming
  loops in `cli/`), which already owns the `tokio` loop and the SSE client; the
  application use cases keep the **pure, testable** steps — encode-one-chunk
  (resample/VAD/WAV) and the transcript-only SSE drive — over the ports. This
  avoids threading a capture-stream associated type through the whole Facade
  while keeping the producer/queue out of the use cases.
- **Consumer (sequential, ordered).** The use case / driving adapter loops
  `recv → resample 16 kHz mono → VAD gate → encode WAV → POST SSE → present
  transcript`, racing `Ctrl-C`. While the consumer POSTs chunk N, the producer
  keeps capturing N+1, N+2… into the queue, so capture is never blocked.
- **Hybrid backpressure.** The bounded `mpsc` provides backpressure to the
  producer thread; when the consumer falls behind, the `mpsc` fills, the producer
  blocks, and the native ring grows until the `cap_secs` ceiling, beyond which
  the RT callback drops the oldest frames. So: **zero loss up to a ~60 s
  backlog, bounded memory, drop-oldest only under sustained overload** (logged).
- **Scope.** Applies to `transcribe --stream` and `realtime` (both shared the
  serial loop). The capture-and-gate step (resample/VAD/encode) is reused from
  the existing shared `capture` module; only its *source* changes from one-shot
  to the continuous stream.
- A server-side `tts=false` toggle (ADR-0014 future work) would further shrink
  per-chunk consumer time, but the decoupling makes consumer latency non-blocking
  regardless.

### Consequences

- Good: captured audio is no longer dropped in the gap; `transcribe --stream`
  and `realtime` stop "eating words." Capture and the SSE POST never contend.
- Good: bounded memory; transcripts stay ordered (single consumer).
- Good: `tokio` stays out of the ports; the producer/queue live in the adapter,
  the consumer in the use case.
- Bad: a sustained-slow server still drops the oldest audio past the ~60 s cap
  (a `tracing` warning surfaces it); a dedicated producer thread per session.
- Neutral: word-boundary cuts remain a smaller residual; chunk overlap / VAD-
  aligned cutting is a possible later refinement. `cap_secs` and `chunk_secs`
  are config knobs (ADR-0006).
