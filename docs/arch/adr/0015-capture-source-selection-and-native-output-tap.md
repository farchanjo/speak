---
status: accepted
date: 2026-06-27
deciders: [farchanjo]
consulted: []
informed: []
---

# Capture source selection and native macOS output tap

## Context and Problem Statement

`speak` captures audio only from an **input** device (microphone / line-in) via
`AVAudioEngine.inputNode`, optionally pinned to a non-default input
(ADR-0011) and reduced to one channel (ADR-0013). Users also need to capture
what the host is **playing** — the sound-card / system output — for example to
transcribe a call or a video, or to record system audio.

The constraint is hardware-independence: a typical audio interface ("a
controladora") has **no hardware loopback**, so the output cannot be re-captured
by listening to an input. The capture must therefore be obtained **directly on
the host in software**, and it should apply uniformly to the live commands
(`transcribe --stream`, `realtime`) and to `record`.

macOS offers two software routes to system/output audio:

1. **Native Core Audio process/output tap** (macOS 14.4+):
   `AudioHardwareCreateProcessTap(CATapDescription)` creates a tap on process /
   device output; embedding the tap in a private aggregate device
   (`AudioHardwareCreateAggregateDevice`) exposes the tapped audio as a capture
   stream. Driver-free, "direct from the PC". Requires macOS 14.4+ and may
   require a one-time audio-capture permission.
2. **Virtual-loopback device** (BlackHole, Loopback, or an aggregate device):
   the user routes output → a virtual device that presents as an **input**;
   `speak` then captures it through the existing input path. Works on older
   macOS but requires installing/configuring a driver.

## Decision Drivers

- Capture host output with **no hardware loopback**, directly on the machine.
- A single selection model shared by `transcribe --stream`, `realtime`, and
  `record` — not three bespoke flags.
- Native where possible (no driver), with a documented escape hatch where not.
- Zero media process-exec; the only `objc2`/CoreAudio code stays in the
  CoreAudio adapter, behind the `AudioSource` port (ADR-0001 / ADR-0003).
- Layered config with `SPEAK_*` overrides and reported origin (ADR-0006).

## Considered Options

- **Option A** — Native Core Audio tap only. Cleanest "direct" capture, but
  hard-fails on macOS < 14.4 and where permission is denied.
- **Option B** — Virtual-loopback only (document BlackHole, reuse the input
  path). Near-zero code, but never "direct from the PC" and needs a driver.
- **Option C** — **Both**: a `CaptureSource` Strategy with `input` / `output`
  variants. `output` uses the native tap as the primary implementation; the
  routed virtual-loopback device is a documented fallback that simply reuses the
  `input` source (`--source input -d <loopback>`), needing no special code.

## Decision Outcome

Chosen option: **Option C — both**, native primary + documented BlackHole
fallback.

- **Domain.** Add a pure `CaptureSource` value object (Strategy selector):
  `Input { device, channel }` and `Output { device, channel }`, each with an
  optional `AudioDeviceId` and an optional 0-based capture channel. It carries
  no framework type.
- **Port.** The `AudioSource` port gains a source-aware capture
  (`capture(source, secs)`); the `Input` arm is the existing behavior, the
  `Output` arm is the native tap. No `objc2` type crosses the boundary.
- **Adapter (macOS).** The CoreAudio adapter implements `Output` capture with a
  Core Audio HAL tap: build a `CATapDescription` (system mix by default, or the
  selected output device), `AudioHardwareCreateProcessTap`, embed it in a
  private `AudioHardwareCreateAggregateDevice`, capture that aggregate device's
  stream, then destroy the aggregate device and the tap. Channel selection
  reuses the existing single-channel pick (ADR-0013). The path lives behind the
  `cfg(target_os = "macos")` gate; non-macOS and pre-14.4 return a clear error
  pointing at the fallback. This is the lone new `unsafe` FFI surface and is
  validated on-device (a tap on real output, with permission), per the
  debugger-grounded discipline — never assumed from headers.
- **Fallback.** A routed virtual-loopback device (BlackHole / aggregate) appears
  as an input device and is captured with `--source input -d <id>`; documented
  in CLAUDE.md and `config init`, requiring no code beyond what already
  enumerates input devices.
- **CLI.** `--source <input|output>` (short flag per ADR-0012, default `input`)
  on `transcribe --stream`, `realtime`, and `record`; the active source's device
  is `-d/--device` and its channel is `-I/--input-channel`.
- **Config.** Add `[audio.capture].source` (`input` | `output`, default
  `input`) plus the output device/channel knobs, each with a `SPEAK_*` override
  and a default; `config show` reports origin (ADR-0006). The `input` source
  keeps reading `[audio.input]` unchanged.
- **Errors.** A denied permission or an unavailable tap fails with an actionable
  message (what was denied, how to grant, the BlackHole fallback) — never a
  panic or a silent empty capture (FR-9).

### Consequences

- Good: system/output capture works "direct from the PC" on macOS 14.4+ with no
  driver, and still works on older macOS or under restrictive permissions via
  the routed-loopback fallback — without forking the pipeline.
- Good: one `CaptureSource` Strategy serves all three capture commands; the
  input path is unchanged and regression-safe.
- Good: the only new framework/`unsafe` code is the tap, confined to the
  CoreAudio adapter behind the `AudioSource` port.
- Bad: the native tap is non-trivial, version-gated CoreAudio FFI that must be
  verified on real hardware with capture permission; it is implemented and
  validated as a distinct phase after the source-selection plumbing and the
  BlackHole path ship.
- Bad: output capture may trigger a one-time OS permission prompt; headless or
  unattended runs must surface the denial clearly rather than hang.
- Neutral: extends ADR-0007 (device routing) on the capture side and reuses
  ADR-0011 / ADR-0013 (device + channel selection) for both sources.
