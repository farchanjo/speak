---
status: accepted
date: 2026-06-28
deciders: [farchanjo]
consulted: []
informed: []
---

# VAD-segmented streaming chunks (cut on silence, not a fixed grid)

## Context and Problem Statement

Live streaming (`transcribe --stream`, `translate --stream`, `realtime`) sliced
captured audio on a **fixed time grid**: the producer (ADR-0017) drained exactly
`chunk_secs` (default 5 s) of audio and POSTed each block to Whisper as an
independent island. Two field-reported failures followed ("corta partes da fala,
hora nem chega nada, ora funciona"):

1. **Words cut at boundaries.** A word or sentence straddling the 5 s edge is
   split — Whisper sees half a word at the tail of block N and half at the head
   of block N+1, and garbles or drops both. No overlap, no awareness of where
   speech actually is. ADR-0017 flagged this as a known "secondary" defect.
2. **Whole quiet blocks vanish.** The VAD gate (`capture.rs::gate_chunk`) ran on
   the **average RMS of the entire 5 s block** against `silence_threshold_db`
   (−38 dBFS). A block that is mostly quiet with a short soft line averages below
   the floor → the *entire block is dropped*, soft dialogue included.
3. **Intermittent.** Whether a given utterance survives depends on where the
   arbitrary 5 s grid happens to fall against the speech — a lottery.

```
audio:      …word1 word2 wo│rd3 word4 (soft line)│ …
fixed 5s:   [──── block N ──]│[──── block N+1 ───]│   ← "wo|rd3" split; soft block avg < floor → dropped
```

## Decision Drivers

- Never split a word — cuts must land in the silence between utterances.
- Never drop a real-but-soft line — gating must be per-utterance, not per-grid.
- Give Whisper complete utterances (better accuracy, correct boundaries).
- Bound memory and keep capture decoupled (ADR-0017 invariants intact).
- Keep `--no-vad` meaning "send everything, drop nothing".

## Considered Options

1. **Overlap + dedup** — keep the fixed grid but prepend ~1.5 s of the previous
   block, then deduplicate the repeated transcript text. Rejected: text-level
   dedup of Whisper output across overlaps is fragile (paraphrase, casing,
   punctuation drift) and still grid-bound.
2. **VAD-segmented chunks (chosen)** — accumulate audio continuously and cut on a
   trailing-silence pause (or a hard cap). Cuts land in pauses, soft lines stay
   whole, Whisper gets complete utterances.
3. **Bigger fixed blocks** — halve the cut frequency only; the same two defects
   remain. Kept as the `--no-vad` fallback, not the default.

## Decision Outcome

Chosen option: **Option 2 — VAD-segmented chunks**, with fixed slicing retained
as the `--no-vad` path.

- The producer (`coreaudio/macos/stream.rs`) gains a pure `Segmenter` state
  machine. Fed incremental interleaved hops drained from the ring every
  `POLL_MS`, it:
  - marks each hop speech/silence by RMS vs a linear `floor`
    (`silence_threshold_db` → linear, or `--vad-floor`);
  - retains a `PRE_ROLL_SECS` lead-in so the first consonant is not clipped;
  - **flushes** the segment when trailing silence reaches `HANG_SECS` (a natural
    pause) **or** the buffer reaches `MAX_SEGMENT_SECS` (hard cap for unbroken
    speech), trimming trailing silence to `POST_ROLL_SECS`;
  - **drops** a segment whose speech content is below `MIN_SPEECH_SECS` (noise
    blip), so pure noise never POSTs.
  - Consts: `MIN_SPEECH_SECS=0.25`, `HANG_SECS=0.7`, `MAX_SEGMENT_SECS=14.0`,
    `PRE_ROLL_SECS=0.3`, `POST_ROLL_SECS=0.3` (alongside `CHANNEL_CHUNKS`/
    `POLL_MS`; not config knobs).
- `CoreAudio::capture_stream` now takes a `SegmentParams { vad, floor, chunk_secs,
  cap_secs }`; the three CLI callers build it from the resolved stream options.
  When `vad` is off the producer keeps the ADR-0017 fixed `chunk_secs` slicing
  (no audio dropped).
- The producer is now the **single VAD authority**. The downstream
  `StreamTranscribeUseCase::encode` no longer re-gates (a second whole-segment RMS
  gate would re-introduce defect #2 on a soft utterance).

Hexagonal boundaries unchanged: the `Segmenter` is adapter-internal,
`SegmentParams` is a plain POD on the `coreaudio` adapter surface (no domain or
framework type leaks), and the application layer is untouched apart from removing
the redundant encode gate.

### Consequences

- **Good** — boundary words are no longer split (cuts fall in pauses); soft lines
  survive (per-utterance, trimmed gating); Whisper receives whole utterances, so
  transcripts are more accurate; pure-noise segments never POST.
- **Bad / trade-offs** — first-byte latency now tracks the speaker's pause
  cadence plus `HANG_SECS`, not a fixed 5 s tick (usually faster for short lines,
  occasionally longer for a long unbroken sentence up to `MAX_SEGMENT_SECS`). A
  monologue with no pause is still cut once at the cap. The five consts are fixed;
  if they need per-deployment tuning they can later graduate to `[audio.input]`
  config.
- `realtime` shares the segmented producer; its own `gate_chunk` gate is left in
  place (segments are trimmed to speech, so it passes them) — a later cleanup can
  unify it.

## More Information

Extends ADR-0017 (decoupled capture) and ADR-0014 (transcript-only stream);
composes with ADR-0018 (pipelined consumer). The `--vad-floor`/`--no-vad` flags
and `[audio.input].silence_threshold_db` (ADR-0011) now feed the producer rather
than the per-chunk encode gate.
