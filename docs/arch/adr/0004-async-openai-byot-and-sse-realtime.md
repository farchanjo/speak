---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# async-openai (_byot) client and SSE realtime stream

## Context and Problem Statement

`speak` targets an OpenAI-compatible speech server (v2.3) whose
`/v1/audio/speech` endpoint accepts the standard OpenAI fields plus an extended
set the typed `CreateSpeechRequest` cannot express: `instruct` (voice design),
`language`, `voice=<clone>`, `ref_text`, and generation parameters
(`num_step`/`steps`, `guidance_scale`, `t_shift`, `layer_penalty_factor`,
`position_temperature`, `class_temperature`, `denoise`, `preprocess_prompt`,
`postprocess_output`, `audio_chunk_duration`, `audio_chunk_threshold`). The
server is also adding a streaming endpoint `POST /v1/realtime/translate` that
emits Server-Sent Events. We need an HTTP client that speaks the standard
OpenAI audio API for the common cases and the extended schema for voice design,
cloning, and tuning, plus an SSE consumer for the realtime stream.

## Decision Drivers

- Reuse a maintained OpenAI-compatible client rather than hand-rolling HTTP.
- Send fields the typed request cannot express, without forking the crate.
- One warm, pooled HTTP client reused across every call (including each
  realtime iteration).
- Consume the realtime SSE schema as a small, typed module that does not block
  the rest of the client if the endpoint is absent.

## Considered Options

- Option A — `async-openai` 0.41.x configured with
  `OpenAIConfig::with_api_base(host).with_api_key(key)`; typed requests for the
  standard endpoints and the crate's `_byot` ("bring your own types") methods
  for the extended speech request; `eventsource-stream` for the realtime SSE.
- Option B — Raw `reqwest` for everything, hand-modelling every request/response.
- Option C — Fork `async-openai` to add the extra fields to its typed structs.

## Decision Outcome

Chosen option: "Option A".

- Standard endpoints (`/v1/models`, `/v1/audio/transcriptions`,
  `/v1/audio/translations`, voice CRUD) use typed requests.
- The single CANONICAL generation key for step count is `num_step` (the CLI
  accepts `steps` as an alias that normalizes to `num_step`). `num_steps` is
  **not** a valid key and is rejected by the request Builder's field guard.
- The `Translator` port has two interchangeable **Strategy** implementations
  (FR-7 / FR-8): (a) the default OpenAI-audio strategy, implemented by the
  `openai` adapter over `/v1/audio/translations`, which translates audio to
  English via Whisper; and (b) a chat-MT strategy for an arbitrary `--to`
  target, implemented by a separate `chatmt` adapter that POSTs to the
  non-OpenAI `[general].translate_url` endpoint with `[general].translate_model`
  using the same warm `reqwest` pool. The composition root selects the strategy:
  English target or absent `translate_url` -> Whisper translate; non-English
  target with `translate_url` set -> chat-MT; non-English target without
  `translate_url` -> degrade to the source transcript with a clear notice.
- The extended `/v1/audio/speech` request (voice-design `instruct`, `language`,
  `voice=clone`, `ref_text`, and all generation parameters) is sent through the
  `_byot` methods with a `speak`-owned serde type built by a fluent Builder.
  The native `/tts` endpoint uses the same builder output.
- The realtime client consumes `POST /v1/realtime/translate` as SSE frames
  `{type: transcript|translation|audio|done|error, text?, audio_b64?, format?,
  seq?}` via `eventsource-stream`, decoded into a typed `RealtimeFrame` enum in
  the `adapters/sse` module behind the `RealtimeStream` port. The feature is
  flag-guarded at **runtime**, not compile time: the realtime use case probes
  the server (a `POST /v1/realtime/translate` capability check, also surfaced by
  `/v1/models`) and, when the endpoint answers, consumes SSE frames; when it is
  absent or errors, it falls back to the chunked ASR -> MT -> TTS pipeline. A
  runtime probe (rather than a compile-time Cargo feature) is chosen so one
  prebuilt binary works against servers with or without the endpoint; the
  `eventsource-stream` dependency is always linked. An optional `realtime-sse`
  Cargo feature may gate the parser out for size-constrained builds, but the
  default binary always carries it and decides at runtime.
- A single `async-openai` client (warm keep-alive, tuned reqwest pool) is built
  once in the composition root and shared by every adapter call.

### Consequences

- Good: standard cases stay type-safe; the extended speech request and tuning
  knobs are expressible without forking; the realtime consumer is isolated and
  optional.
- Good: the OpenAI base URL points at any compatible server via `--host`.
- Bad: `_byot` payloads are validated by the server, not the compiler, so the
  request Builder must guard field names and the voice-design tag vocabulary
  in the domain layer before sending.
