# Research: native macOS Core Audio output tap (Phase 2, T011-T013)

Status: **API confirmed, implementation planned, on-device validation pending.**
The whole tapping API is already linked via `objc2-core-audio` 0.3.2 with the
`AudioHardware` feature (already enabled in `Cargo.toml`). No new direct
dependency is required for the Core Audio side; Foundation collection helpers
need a few extra `objc2-foundation` features (below).

## Confirmed API surface (objc2-core-audio 0.3.2, `AudioHardware`)

- `CATapDescription`
  - `initStereoGlobalTapButExcludeProcesses(&NSArray<NSNumber>)` — a **stereo
    global tap of the whole system output**, excluding the listed PIDs; pass an
    **empty array** to tap everything the host is playing (the target case).
    Mono and per-process / per-device variants also exist
    (`initMonoGlobalTapButExcludeProcesses`, `initStereoMixdownOfProcesses`,
    `initWithProcesses:andDeviceUID:withStream:` for a specific output device).
  - `setName(&NSString)`, `setUUID(&NSUUID)`, `setPrivate(bool)`,
    `setMuteBehavior(CATapMuteBehavior)`, `UUID() -> NSUUID`.
- `AudioHardwareCreateProcessTap(Option<&CATapDescription>, *mut AudioObjectID) -> OSStatus`
- `AudioHardwareDestroyProcessTap(AudioObjectID) -> OSStatus`
- `AudioHardwareCreateAggregateDevice(&CFDictionary, NonNull<AudioObjectID>) -> OSStatus`
- `AudioHardwareDestroyAggregateDevice(AudioObjectID) -> OSStatus`
- Aggregate-description keys (all `&CStr`, i.e. plain dictionary string keys):
  `kAudioAggregateDeviceUIDKey "uid"`, `kAudioAggregateDeviceNameKey`,
  `kAudioAggregateDeviceIsPrivateKey`, `kAudioAggregateDeviceTapAutoStartKey`,
  `kAudioAggregateDeviceTapListKey "taps"`, sub-tap entry key
  `kAudioSubTapUIDKey "uid"`, `kAudioSubTapDriftCompensationKey`.
- The tap's UID equals `CATapDescription.UUID().UUIDString()` (no property read
  needed); `kAudioTapPropertyUID` is the fallback selector if required.

## Implementation sequence (`src/adapters/coreaudio/macos/tap.rs`)

`capture_output(device: Option<u32>, channel: Option<u16>, secs: f64) -> Result<PcmBuffer>`:

1. `let excludes = NSArray::<NSNumber>::from_retained_slice(&[]);`
2. `let desc = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &excludes);`
   `desc.setName(&NSString::from_str("speak output tap")); desc.setPrivate(true);`
   (a `device`-scoped variant uses `initWithProcesses:andDeviceUID:withStream:`).
3. `let mut tap_id = 0; let st = AudioHardwareCreateProcessTap(Some(&desc), &mut tap_id);`
   status-check; `0` is success.
4. `let uid: Retained<NSString> = desc.UUID().UUIDString();`
5. Build the aggregate description `NSDictionary<NSString, AnyObject>`:
   - `"uid"` → a fresh unique UID string (`NSUUID::new().UUIDString()`),
   - `"name"` → `"speak-aggregate"`,
   - `kAudioAggregateDeviceIsPrivateKey` → `NSNumber::numberWithBool(true)`,
   - `kAudioAggregateDeviceTapAutoStartKey` → `NSNumber::numberWithBool(true)`,
   - `"taps"` → `NSArray::from_retained_slice(&[subtap])` where `subtap` is
     `NSDictionary{ "uid" → uid }`.
   Heterogeneous values: upcast each `Retained<T>` to `Retained<AnyObject>` (e.g.
   `Retained::cast`) and use `NSDictionary::from_retained_objects(&keys, &vals)`.
6. Toll-free-bridge the `NSDictionary` to `&CFDictionary`
   (`&*(Retained::as_ptr(&dict).cast::<CFDictionary>())`) and call
   `AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut agg_id))`.
7. **Reuse the existing capture engine**: `engine::capture(Some(AudioDeviceId(agg_id)), secs)`
   — the aggregate device presents the tapped output as an input stream, so the
   existing `AVAudioEngine` input path records it. Channel selection stays in the
   application (`pick_input_channel`).
8. Teardown (always, even on error — RAII guard):
   `AudioHardwareDestroyAggregateDevice(agg_id); AudioHardwareDestroyProcessTap(tap_id);`
9. Wire: override `AudioSource::capture_for` on `CoreAudio` so `Output` routes to
   `tap::capture_output`, replacing the Phase-1 placeholder error.

## Cargo features to add

- `objc2-foundation`: `NSArray`, `NSDictionary`, `NSValue` (NSNumber), `NSUUID`.
- `objc2-core-audio`: already has `AudioHardware`.
- `objc2-core-foundation`: `CFDictionary` is pulled by `AudioHardware`; the TFB
  cast avoids building `CFArray`/`CFNumber` directly.

## Why this is on-device (T013), not headless

Per project doctrine (CLAUDE.md §7), native runtime behavior is verified by
running and reading the truth, never assumed from headers. Two things only a real
Mac with audio playing can confirm:

1. **TCC permission** — the first `AudioHardwareCreateProcessTap` may require an
   audio-capture authorization; a denied/unprompted run must surface the
   actionable error (FR-9), not hang. Headless/cron runs cannot grant it.
2. **Aggregate-device capture** — that `AVAudioEngine.inputNode` bound to the
   private tap aggregate actually yields the playing audio (format, channel
   count, non-silence).

Validation loop: `make build-dbg` → run `speak transcribe --stream --source output`
with audio playing → `make debug-attach` / lldb to read `tap_id`, `agg_id`,
`OSStatus`, and the captured RMS. Heterogeneous-`NSDictionary` construction is the
main compile-iteration point and is fastest to settle with the compiler in the
loop.

## Interim

Output capture works **today** via the BlackHole fallback (FR-6):
`speak transcribe --stream --source input -d <blackhole-id>` — a routed
virtual-loopback device is an ordinary input device. The native tap removes the
driver requirement; it does not unlock new capability the fallback lacks.
