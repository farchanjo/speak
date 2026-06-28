---
status: accepted
date: 2026-06-28
deciders: [farchanjo]
consulted: []
informed: []
---

# Pipelined in-flight SSE consumer for streaming capture

## Context and Problem Statement

ADR-0017 decoupled native capture from the SSE consumer with a bounded channel
so capture never stalls. But the **consumer** stayed serial: `transcribe
--stream` and `translate --stream` (`cli/{transcribe,translate}.rs`) call
`drive_one` inside a `select!` and process **one chunk at a time** — `recv →
encode → POST → drain SSE to done → present`, then `recv` the next chunk. The
SSE round trip includes the server's ASR (and TTS re-voicing, still synthesized
even for `translate=false`). When that round trip takes longer than `chunk_secs`
(default 5 s), the consumer can never keep pace with realtime:

```
chunk_secs = 5s, CHANNEL_CHUNKS = 8 (40s), ring cap = 60s  ⇒  ~100s of hidden buffering
```

The two queues (ring + channel) back up, latency grows to ~100 s, then the ring
drops the oldest audio — the user perceives the stream "parando" (freezing) and
loses words. The bottleneck is the **serial** consumer: throughput is
`1 / round_trip`, so a round trip ≈ chunk duration means permanent fall-behind.

## Decision Drivers

- Keep pace with realtime when a single SSE round trip ≈ `chunk_secs`.
- Preserve output order — transcripts/translations must print in capture order,
  never interleaved across chunks.
- Bound concurrency (and therefore memory + server load).
- Keep Ctrl-C immediate (ADR-0017's single pinned `ctrl_c()` future).
- Share one implementation across `transcribe --stream` and `translate --stream`;
  leave `realtime` serial (it plays audio back — out-of-order playback is wrong).

## Considered Options

1. **Shrink the buffers only** — drop `CHANNEL_CHUNKS` 8 → 2. Cuts hidden
   latency from 40 s to 10 s but does NOT raise consumer throughput; a slow
   server still falls behind, just with less lag before dropping.
2. **Pipeline in-flight POSTs (chosen)** — overlap up to `MAX_INFLIGHT` chunk
   round trips, presenting completed chunks in capture order via an ordered
   futures queue. Throughput becomes `MAX_INFLIGHT / round_trip`.
3. **Unbounded spawn per chunk** — rejected: no backpressure, no order
   guarantee, unbounded server load.

We adopt **option 2 and also apply option 1** (`CHANNEL_CHUNKS` 8 → 2): with a
pipelined consumer the deep channel only adds latency, so a shallow channel
keeps the stream near-live while the ring (`buffer_secs`) remains the real
backpressure ceiling.

## Decision Outcome

Chosen option: **Option 2 (pipeline in-flight POSTs) + Option 1 (shallow channel)**.

- New shared consumer `cli/stream_pipeline.rs`:
  - `run(capture, presenter, label, build)` drives the session. `build(chunk)`
    encodes one captured chunk (sync, VAD-gated) and returns the future that
    POSTs it and collects the lines to print (`None` = silence/encode-skip).
  - In-flight futures live in a `futures_util::stream::FuturesOrdered`, which
    yields **only the head when it completes** — guaranteeing capture-order
    output even though round trips finish out of order.
  - A `tokio::select!` races: the single pinned `ctrl_c()` (ADR-0017); draining
    one completed chunk (presented in order); and `capture.recv()` — gated off
    once in-flight reaches `MAX_INFLIGHT` (backpressure to the channel/ring) or
    the capture has ended.
  - `collect_chunk(...)` builds the reconnecting SSE stream, drives it via the
    shared `StreamTranscribeUseCase`, and collects only the wanted `FrameKind`
    (Transcript for transcribe, Translation for translate). Per-chunk server
    errors are logged (`tracing::warn`) and never abort the session.
- `MAX_INFLIGHT = 3` is a named const alongside the existing `CHANNEL_CHUNKS` /
  `POLL_MS` consts — not a config knob (matches the ADR-0017 precedent).
- `CHANNEL_CHUNKS` 8 → 2 in `coreaudio/macos/stream.rs`.

Hexagonal boundaries are unchanged: `stream_pipeline` is part of the `cli`
driving adapter; `tokio` stays inside it and the `coreaudio`/`sse` adapters; the
application layer (`StreamTranscribeUseCase`) is reused untouched.

### Consequences

- **Good** — consumer throughput scales to `MAX_INFLIGHT` overlapping round
  trips; with `CHANNEL_CHUNKS = 2` the live stream stays within ~10 s of realtime
  instead of ~100 s. Output order is preserved by `FuturesOrdered`. One shared
  consumer for both streaming commands removes the duplicated `drive_one`/
  `process_chunk` pair.
- **Bad / trade-offs** — within a chunk, transcripts are buffered and printed on
  chunk completion (not incrementally), so the *finest-grained* live feel is
  slightly coarser; acceptable since chunks are short. A sustained slow server
  still eventually backpressures and drops oldest audio (ring `buffer_secs`) —
  pipelining raises the ceiling, it does not make an under-provisioned server
  infinite.
- `realtime` is intentionally left serial (ordered playback).

## More Information

Supersedes nothing; extends ADR-0017. Related: ADR-0014 (transcript-only
stream), ADR-0004 (SSE realtime).
