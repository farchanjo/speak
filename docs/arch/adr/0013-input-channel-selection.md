---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Single input-channel selection for multi-channel capture devices

## Context and Problem Statement

Capturing from a professional audio interface (e.g. an SSL 12) for `realtime` or
`record` produced silence in the ASR pipeline even though the microphone worked.
The interface exposes **16 input channels**; macOS captures all of them, and the
ASR path resamples the multi-channel buffer to **mono by averaging every channel**
(`to_asr_mono16` / `resample(..., ASR_CHANNELS=1)`).

When the mic is wired to a single input (channel 0 = SSL input 1) and the other
15 channels are silent, the average attenuates the live channel by roughly the
channel count. Measured live: channel 0 at −43 dBFS RMS (peak −26 dBFS) became
−55 dBFS RMS after the 16→1 downmix — below the −38 dBFS VAD floor, so every chunk
was gated as silence. The audio that did pass was the lone channel attenuated
~16×. There was no way to tell the client "use input N", only the device.

## Decision Drivers

- Capture a mic on one input of a many-channel interface at full level, without
  the user re-wiring hardware or building an aggregate device.
- Keep the default behaviour (downmix all channels) for ordinary mono/stereo mics.
- Configurable once (a daily-driver interface always uses the same input) and
  overridable per invocation.
- Stay in the hexagon: the selection is a pure operation on the domain
  `PcmBuffer`; no CoreAudio/libav type is involved.

## Considered Options

1. **Lower the VAD floor / `--no-vad` only.** Lets the diluted chunk through but
   the audio is still attenuated ~16×, hurting ASR. A workaround, not a fix.
2. **Auto-pick the loudest channel.** Convenient but magic — it can latch onto a
   noisy idle channel and is unpredictable run to run.
3. **Explicit channel selection before the downmix.** Extract one 0-based channel
   from the captured `PcmBuffer` into a mono buffer, then resample/encode as
   usual. Predictable, opt-in, full level.

## Decision Outcome

Chosen option: **Option 3 (explicit channel selection)**.

- `PcmBuffer::select_channel(n)` extracts one 0-based channel into a mono buffer
  (same sample rate), returning `None` when out of range — a pure domain method.
- `record` and `realtime` accept `-I` / `--input-channel <N>`, and a persistent
  `[audio.input].channel` config key (`SPEAK_AUDIO_INPUT_CHANNEL`). The flag
  overrides the config; both default to `None` = downmix all channels (unchanged
  behaviour). The use cases apply the selection right after capture, before the
  conform/resample step. An out-of-range channel is a clear error naming the
  device's channel count.

### Consequences

- Good: a mic on SSL input 1 works with `realtime -d <ssl> -I 0` (or once via
  `[audio.input].channel = 0`), captured at full level so the VAD gate behaves.
- Good: pure, unit-tested domain operation; zero new framework surface.
- Neutral: the user must know which input their mic is on (`speak record -D <id>
  -d 3` + a level check per channel identifies it).
- Constraint: selection happens after the full multi-channel capture (the device
  still delivers all channels); it is a post-capture extraction, not a
  hardware-level channel mask.

## Verification

- Unit: `PcmBuffer::select_channel` extracts the right interleaved channel and
  rejects out-of-range; the `record` use case selects a channel and errors on an
  out-of-range index.
- Live (macOS): `record -D <multichannel> -I 0` yields a mono file at full level;
  `realtime -d <multichannel> -I 0` transcribes a mic on input 1.
