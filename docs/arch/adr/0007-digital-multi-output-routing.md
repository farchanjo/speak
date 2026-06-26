---
status: accepted
date: 2026-06-26
deciders: [farchanjo]
consulted: []
informed: []
---

# Digital multi-output audio routing (fan-out)

## Context and Problem Statement

Some workflows need the same synthesized or re-voiced audio to play out of more
than one output device at once — for example a local monitor plus a virtual
device captured by a meeting app. `speak say` and `speak realtime` therefore
accept a repeatable `--output-device`. We need a routing model that sends one
decoded PCM stream to one or many devices while keeping the constitution's
in-process, fully-digital, no-exec rule.

## Decision Drivers

- Honour a single `--output-device` and a fan-out to many, from one decode.
- Stay entirely in-process and digital — no analog loopback, no child process.
- Pin output to specific `AudioDeviceID`s, not just the system default.
- Keep routing inside the `coreaudio` adapter behind the `AudioSink` port.

## Considered Options

- Option A — One decode/resample, then fan the PCM out to N native engines:
  one `AVAudioEngine` per target `AudioDeviceID` (each `AVAudioPlayerNode ->
  mainMixerNode -> outputNode` pinned to its device), or an aggregate device,
  all driven by the `AudioSink` port.
- Option B — Shell out to a system audio router / virtual cable.
- Option C — Single default-device output only; no fan-out.

## Decision Outcome

Chosen option: "Option A".

- The `coreaudio` adapter enumerates devices via CoreAudio
  (`kAudioHardwarePropertyDevices`) and exposes them through `speak devices`
  (`--json` for machine consumption).
- The `AudioSink` port accepts a set of **platform-neutral target device
  selectors** (device names or opaque identifier strings), keeping the port
  contract free of any CoreAudio type — `AudioDeviceID` (a CoreAudio `u32`) never
  crosses the port boundary. The `coreaudio` adapter resolves each selector to its
  `AudioDeviceID`: with one target it pins a single `AVAudioEngine` to that
  `AudioDeviceID`; with many it builds one engine per device (or an aggregate
  device) and schedules the same decoded `AVAudioPCMBuffer`s on each, so a single
  decode feeds all outputs.
- Selection is by `--output-device` (repeatable on `say` and `realtime`) or the
  `[audio.output].device` config — which itself accepts either a single device
  name or a **list** of names as the default fan-out set, so the multi-device
  set is expressible in TOML as well as on the CLI (ADR-0006); the repeatable
  flag overrides the config default per call. Volume maps to
  `mainMixerNode.outputVolume`.
- The entire path — server bytes -> libav decode/resample -> native mixer(s) ->
  device(s) — is digital end-to-end; nothing is exec'd and no analog stage is
  involved.

### Consequences

- Good: true multi-destination playback from one decode; explicit device
  pinning; honours the no-exec, all-digital rule; `devices` makes IDs
  discoverable.
- Good: routing is confined to the `coreaudio` adapter behind `AudioSink`, so
  the use cases stay device-agnostic.
- Bad: N engines cost N times the output buffers and add clock-drift risk across
  independent devices (an aggregate device mitigates this); macOS-only today.
