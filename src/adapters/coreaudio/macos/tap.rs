//! Native macOS Core Audio output tap (ADR-0015, Phase 2 / T011-T012).
//!
//! Captures the host's system output ("what the PC is playing") with no hardware
//! loopback. A stereo global process tap (`CATapDescription` →
//! `AudioHardwareCreateProcessTap`) is embedded in a private, auto-starting
//! aggregate device (`AudioHardwareCreateAggregateDevice`); that aggregate
//! presents the tapped output as an input stream, so the existing `AVAudioEngine`
//! capture ([`super::engine::capture`]) records it. macOS 14.4+; the first tap
//! may trigger an audio-capture (TCC) authorization.
//!
//! Lifecycle is RAII: [`ProcessTap`] and [`AggregateDevice`] destroy themselves
//! on drop, in reverse construction order (aggregate first, then tap), on every
//! exit path including capture errors.

use std::ffi::CStr;
use std::ptr::NonNull;

use anyhow::{Result, bail};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, Message};
use objc2_core_audio::{
    AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectID,
    CATapDescription, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioSubTapUIDKey,
};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::AudioDeviceId;

use super::engine;

/// A heterogeneous Core Audio configuration dictionary (string keys, object vals).
type ConfigDict = NSDictionary<NSString, AnyObject>;

/// Capture `secs` seconds of the host system output as interleaved float PCM.
///
/// `device` (a specific output `AudioDeviceID`) is reserved for a future
/// device-scoped tap; v1 taps the whole system output mix. `channel` is applied
/// by the caller after capture (ADR-0013), so the full stereo capture is
/// returned here. The tap + aggregate device are torn down before returning.
pub fn capture_output(_device: Option<u32>, _channel: Option<u16>, secs: f64) -> Result<PcmBuffer> {
    let tap = ProcessTap::global()?;
    let aggregate = AggregateDevice::wrapping(&tap)?;
    engine::capture(Some(AudioDeviceId(aggregate.id)), secs)
}

/// A system-output process tap, destroyed on drop.
struct ProcessTap {
    id: AudioObjectID,
    /// The tap's UID (its description UUID), referenced by the aggregate sub-tap.
    uid: Retained<NSString>,
}

impl ProcessTap {
    /// Create a stereo tap of the entire system output (no excluded processes).
    fn global() -> Result<Self> {
        // SAFETY: documented Core Audio tapping contract; the description is a
        // stereo global tap with no excluded PIDs (the whole system output mix).
        unsafe {
            let excludes = NSArray::<NSNumber>::from_retained_slice(&[]);
            let desc = CATapDescription::initStereoGlobalTapButExcludeProcesses(
                CATapDescription::alloc(),
                &excludes,
            );
            desc.setName(&NSString::from_str("speak output tap"));
            desc.setPrivate(true);
            let uid = desc.UUID().UUIDString();
            let mut id: AudioObjectID = 0;
            let st = AudioHardwareCreateProcessTap(Some(&*desc), &raw mut id);
            if st != 0 || id == 0 {
                bail!(
                    "AudioHardwareCreateProcessTap failed (OSStatus {st}); host-output capture \
                     needs macOS 14.4+ and audio-capture permission — grant it, or route the \
                     output to a virtual-loopback device (`--source input -d <id>`)"
                );
            }
            Ok(Self { id, uid })
        }
    }
}

impl Drop for ProcessTap {
    fn drop(&mut self) {
        // SAFETY: best-effort teardown of a tap we created.
        unsafe {
            let _ = AudioHardwareDestroyProcessTap(self.id);
        }
    }
}

/// A private aggregate device embedding a tap, destroyed on drop.
struct AggregateDevice {
    id: AudioObjectID,
}

impl AggregateDevice {
    /// Build a private, auto-starting aggregate device wrapping `tap`.
    fn wrapping(tap: &ProcessTap) -> Result<Self> {
        // SAFETY: the description uses the documented aggregate + sub-tap keys;
        // the NSDictionary is toll-free-bridged to CFDictionary for the call.
        unsafe {
            let desc = aggregate_description(tap);
            let cf: &CFDictionary = &*Retained::as_ptr(&desc).cast::<CFDictionary>();
            let mut id: AudioObjectID = 0;
            let st = AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut id));
            if st != 0 || id == 0 {
                bail!("AudioHardwareCreateAggregateDevice failed (OSStatus {st})");
            }
            Ok(Self { id })
        }
    }
}

impl Drop for AggregateDevice {
    fn drop(&mut self) {
        // SAFETY: best-effort teardown of an aggregate device we created.
        unsafe {
            let _ = AudioHardwareDestroyAggregateDevice(self.id);
        }
    }
}

/// Build the aggregate-device description: private + auto-start, one sub-tap.
///
/// # Safety
/// Builds Foundation objects; callers pass the result straight to
/// `AudioHardwareCreateAggregateDevice` via toll-free bridging.
unsafe fn aggregate_description(tap: &ProcessTap) -> Retained<ConfigDict> {
    // Sub-tap entry: { "uid": <tap uid> }.
    let sub_key = key(kAudioSubTapUIDKey);
    let sub_tap: Retained<ConfigDict> =
        NSDictionary::from_retained_objects(&[&*sub_key], &[any(tap.uid.clone())]);
    let taps = NSArray::from_retained_slice(&[sub_tap]);

    let agg_uid = NSUUID::new().UUIDString();
    let k_uid = key(kAudioAggregateDeviceUIDKey);
    let k_name = key(kAudioAggregateDeviceNameKey);
    let k_private = key(kAudioAggregateDeviceIsPrivateKey);
    let k_autostart = key(kAudioAggregateDeviceTapAutoStartKey);
    let k_taps = key(kAudioAggregateDeviceTapListKey);

    let keys = [&*k_uid, &*k_name, &*k_private, &*k_autostart, &*k_taps];
    let values = [
        any(agg_uid),
        any(NSString::from_str("speak-aggregate")),
        any(NSNumber::numberWithBool(true)),
        any(NSNumber::numberWithBool(true)),
        any(taps),
    ];
    NSDictionary::from_retained_objects(&keys, &values)
}

/// An `NSString` for a Core Audio `&CStr` dictionary key.
fn key(name: &CStr) -> Retained<NSString> {
    NSString::from_str(name.to_str().unwrap_or_default())
}

/// Upcast a retained object to `AnyObject` for a heterogeneous collection.
fn any<T: Message>(obj: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an `AnyObject`; this is a pure upcast.
    unsafe { Retained::cast_unchecked::<AnyObject>(obj) }
}
