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

## On-device validation outcome (2026-06-27)

Implemented and verified on macOS 26.5 (M-series) with the real hardware in the
loop. Findings that changed the design:

1. **IOProc, not AVAudioEngine.** The first cut reused the existing
   `AVAudioEngine` input capture by swapping the system default input to the tap
   aggregate (ADR-0011 pattern). It captured the **real default input device**
   (a 16-channel SSL 12), not the tap — the default-input swap does not redirect
   `AVAudioEngine` to a private aggregate. Fixed by reading the aggregate
   **directly by `AudioObjectID`** via `AudioDeviceCreateIOProcID` +
   `AudioDeviceStart`/`Stop` (the fn-pointer variant, `*mut c_void` client data —
   avoids `dispatch2`). After the fix the IO proc reads the correct **2-channel**
   tap aggregate.
2. **Construction is correct.** `AudioHardwareCreateProcessTap`,
   `AudioHardwareCreateAggregateDevice`, `AudioDeviceCreateIOProcID`, and
   `AudioDeviceStart` all return `OSStatus 0`; the IO proc fires and accumulates
   the full frame count from the right device.
3. **The remaining blocker is TCC, not code.** Captured buffers are **all-zero**
   (max_abs 0). The macOS signature of a process tap **without the
   `kTCCServiceAudioCapture` authorization**: the tap runs but is muted. Verified
   against `~/Library/Application Support/com.apple.TCC/TCC.db` — apps that tap
   successfully (Discord, Rogue Amoeba) hold `kTCCServiceAudioCapture=2`; our run
   does not.
4. **A bare CLI can't get the grant.** TCC attributes a CLI's request to the
   parent terminal (or, under automation, the launching app); no inline prompt
   surfaces and `tccutil` cannot add grants. The binary must be a **signed bundle
   subject**: `make app` builds `target/speak.app` with the embedded
   `NSAudioCaptureUsageDescription` (via `build.rs` `-sectcreate`) and the
   `com.apple.security.device.audio-input` entitlement, signed with the
   Apple-Development identity (stable team id). Launching the in-bundle binary
   interactively surfaces the audio-capture prompt; allowing it persists the
   grant. This last step is **interactive and user-environment-specific** — it
   cannot be completed from a headless/automation shell.

5. **CONFIRMED WORKING.** After `make app`, launching the signed bundle via
   LaunchServices — `open target/speak.app --args record -s output …` — fired the
   audio-capture prompt; allowing it landed `ltd.eonf.speak=2` in TCC and the tap
   **captured a 440 Hz tone at mean −24 dBFS / peak −9 dBFS** (2-channel). The
   grant persists by team id.
6. **Responsible-process caveat.** TCC attributes to the *responsible process*.
   `open`-launched `speak.app` is its own responsible subject and holds the grant.
   **Direct-exec from a shell** (`./speak.app/Contents/MacOS/speak …`) makes the
   *parent shell/terminal* responsible — which lacks the grant — so it captures
   silence. Seamless `transcribe --stream --source output` from a terminal needs
   that terminal app granted audio-capture too (run the bundle binary from it once
   and allow), or a future self-`disclaim`-responsibility re-exec (the pattern
   terminal emulators use) so `speak` is always its own subject.

Net: the native tap is **code-complete and confirmed working** (audio captured at
−24 dBFS through the granted bundle). Delivering audio depends only on the OS
`kTCCServiceAudioCapture` grant + the responsible-process being a granted subject
(`make app` → `open` to grant; grant the terminal for direct-exec). The all-zero
case is surfaced as a `tracing` warning pointing at the fix.

## Interim

Output capture works **today** via the BlackHole fallback (FR-6):
`speak transcribe --stream --source input -d <blackhole-id>` — a routed
virtual-loopback device is an ordinary input device. The native tap removes the
driver requirement; it does not unlock new capability the fallback lacks.
