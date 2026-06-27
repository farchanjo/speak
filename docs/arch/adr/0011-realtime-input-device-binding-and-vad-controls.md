---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Realtime/record input-device binding and VAD controls

## Context and Problem Statement

Live `realtime` did nothing when the user spoke into the microphone, and the
cause was two compounding defects in the capture path:

1. **Non-default input device selection was a silent no-op.** `record --device <id>`
   and `realtime --device <id>` (and the `[audio.input].device` config) flow into
   `coreaudio::capture(Some(id), …)`, which built an `AVAudioEngine`, took its
   `inputNode`, and then called `AudioUnitSetProperty(kAudioOutputUnitProperty_CurrentDevice)`
   on the input node's AUHAL. On macOS that property set **returns `noErr` but is
   ignored** for an `AVAudioEngine` input node: the node is bound to the *system
   default input device* the moment it is materialized, and the engine does not
   re-bind it. Verified live: with the system default input set to a 16-channel
   interface (SSL 12), `record --device 182` (a 1-channel "Kenshin Microphone")
   still produced a **16-channel** capture at the interface's format — proof the
   device never switched.

2. **The silence/VAD gate had no run-time escape hatch.** The capture is gated by
   a linear-RMS floor derived from `[audio.input].silence_threshold_db`
   (default −38 dBFS). When the captured stream is quiet (e.g. the wrong device,
   measured at −87 dBFS) **every** chunk is classified as silence and dropped, so
   `realtime` produces no output and offers no obvious diagnosis. The gate could
   only be tuned by editing `config.toml`; there was no CLI flag to lower the floor
   or disable the gate for a single live session.

Native audio is macOS-only (ADR-0001); CoreAudio is the only audio adapter, so the
fix lives entirely in the `coreaudio` adapter plus the `realtime` driving adapter.

## Decision Drivers

- `--device` / `[audio.input].device` MUST actually capture from the chosen device,
  verifiably (the captured channel count/rate must match the device, not the
  previous default).
- The fix must not require the user to change their system audio settings.
- A user must be able to loosen or disable the VAD gate for one invocation without
  editing the config file.
- Stay within the hexagonal contract: no CoreAudio type crosses a port; the change
  is confined to the macOS adapter and the `realtime` flag→`RealtimeOptions` mapping.
- The capture device fix must be mechanically testable **without** live speech.

## Considered Options

1. **Per-node `CurrentDevice` (status quo).** Set the AUHAL `CurrentDevice` on the
   `AVAudioEngine` input node. Rejected — empirically a no-op for input (see above).
2. **A standalone AUHAL input unit** (bypass `AVAudioEngine`, drive the HAL render
   callback directly). Correct and fully general, but a large rewrite of the capture
   path and a second audio-engine code path to maintain.
3. **Temporarily set the HAL system default input device** for the duration of a
   capture, then restore it. `AVAudioEngine.inputNode` binds to the default input,
   so swapping the default before the engine is created makes the engine capture the
   requested device with its native format. Small, localized, and reuses the HAL
   property calls already in the `coreaudio` device adapter.

## Decision Outcome

Chosen option: **Option 3 (temporary HAL default-input swap)** for device binding,
plus **CLI VAD controls** (`--no-vad`, `--vad-floor`).

- The macOS capture path swaps `kAudioHardwarePropertyDefaultInputDevice` to the
  requested `AudioDeviceID` **before** constructing the `AVAudioEngine`, and an RAII
  guard restores the previous default on every exit path (success, error, or
  unwind via `Drop`). `None` (no `--device`) leaves the default untouched. The
  per-node `CurrentDevice` set is removed from the capture path (it is retained for
  output fan-out, where AUHAL output units honor it — ADR-0007).
- `realtime` gains two flags that override the resolved config for one run:
  - `--no-vad` / `-x` — disable the silence gate (every chunk is sent).
  - `--vad-floor <DBFS>` / `-F` — set the silence threshold in dBFS for this run.

### Consequences

- Good: `--device`/`[audio.input].device` works for `record` and `realtime` without
  touching system settings. Acceptance is observable without speech — a capture from
  an N-channel device yields N channels.
- Good: realtime is debuggable in the field (`--no-vad` proves capture→pipeline
  independently of the gate; `--vad-floor` tunes per-room noise).
- Neutral: the system default input is changed for the (sub-second to few-second)
  capture window and restored immediately. Capture is foreground-only and
  single-instance, so there is no concurrent-swap race.
- Risk: a hard crash (SIGKILL) between swap and restore could leave the default
  changed. The RAII guard covers panics and normal errors; only an uncatchable kill
  escapes it, which is acceptable for a developer CLI and recoverable in Sound
  settings.

## Verification

- Unit: `silence_floor` dBFS→linear mapping (existing) plus flag-override wiring.
- Live acceptance (macOS): `speak record --device <1ch-device> -o out.wav --duration 1`
  produces a **1-channel** file; capturing the previous multi-channel default yields
  its channel count — confirming the swap took effect.
- `speak realtime --no-vad --echo` round-trips captured audio with the gate disabled.
