//! Native macOS Core Audio output tap (ADR-0015, Phase 2 / T011-T012).
//!
//! Captures the host's system output ("what the PC is playing") with no hardware
//! loopback. A stereo global process tap (`CATapDescription` →
//! `AudioHardwareCreateProcessTap`) is embedded in a private auto-start aggregate
//! device (`AudioHardwareCreateAggregateDevice`); that aggregate is then read
//! **directly by its `AudioObjectID`** via an `AudioDeviceIOProc`. Reading the
//! aggregate by id (rather than swapping the system default input + an
//! `AVAudioEngine` input node) is what makes the tap audio actually flow — the
//! default-input path binds to the real input device (e.g. an SSL interface),
//! not the private tap aggregate. macOS 14.4+; the first tap may trigger an
//! audio-capture (TCC) authorization.
//!
//! Lifecycle is RAII: the IO proc stops + is destroyed, then the aggregate
//! device and the tap are destroyed, on every exit path including errors.

use std::ffi::CStr;
use std::os::raw::c_void;
use std::ptr::{NonNull, null};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, Message};
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap,
    AudioObjectGetPropertyData, AudioObjectID, AudioObjectPropertyAddress, CATapDescription,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePropertyNominalSampleRate,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioSubTapUIDKey,
};
use objc2_core_audio_types::{AudioBufferList, AudioTimeStamp};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::domain::pcm::PcmBuffer;

/// A heterogeneous Core Audio configuration dictionary (string keys, object vals).
type ConfigDict = NSDictionary<NSString, AnyObject>;

/// Capture `secs` seconds of the host system output as interleaved float PCM.
///
/// `device` (a specific output `AudioDeviceID`) is reserved for a future
/// device-scoped tap; v1 taps the whole system output mix. `channel` is applied
/// by the caller after capture (ADR-0013), so the full multi-channel capture is
/// returned here. The IO proc, aggregate device, and tap are torn down before
/// returning.
pub fn capture_output(_device: Option<u32>, _channel: Option<u16>, secs: f64) -> Result<PcmBuffer> {
    let tap = ProcessTap::global()?;
    let aggregate = AggregateDevice::wrapping(&tap)?;
    // SAFETY: IO-proc lifecycle on a device we created; the sink outlives the
    // start..stop window enforced by the `IoProc` guard.
    unsafe { capture_via_ioproc(aggregate.id, secs) }
}

/// Accumulates interleaved float samples delivered on the Core Audio IO thread.
#[derive(Default)]
struct Sink {
    samples: Vec<f32>,
    channels: u16,
}

/// The IO proc: append the input buffer's float samples to the shared sink.
///
/// # Safety
/// Matches the `AudioDeviceIOProc` ABI; `client` is the `Mutex<Sink>` pointer
/// passed to `AudioDeviceCreateIOProcID`, valid until the proc is destroyed.
unsafe extern "C-unwind" fn io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _in_time: NonNull<AudioTimeStamp>,
    _out: NonNull<AudioBufferList>,
    _out_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the live `Mutex<Sink>` behind the capture's box.
    let sink = unsafe { &*client.cast::<Mutex<Sink>>() };
    // SAFETY: Core Audio hands us a valid buffer list for this IO cycle.
    let list = unsafe { input.as_ref() };
    if list.mNumberBuffers == 0 {
        return 0;
    }
    let buffer = list.mBuffers[0];
    if buffer.mData.is_null() || buffer.mDataByteSize == 0 {
        return 0;
    }
    let count = buffer.mDataByteSize as usize / size_of::<f32>();
    // SAFETY: the tap stream is interleaved float; `count` floats are valid.
    let data = unsafe { std::slice::from_raw_parts(buffer.mData.cast::<f32>(), count) };
    if let Ok(mut guard) = sink.lock() {
        if guard.channels == 0 {
            guard.channels = buffer.mNumberChannels as u16;
        }
        guard.samples.extend_from_slice(data);
    }
    0
}

/// Drive an IO proc on `device` for `secs`, returning the captured PCM.
unsafe fn capture_via_ioproc(device: AudioObjectID, secs: f64) -> Result<PcmBuffer> {
    let rate = unsafe { nominal_sample_rate(device)? };
    let sink: Box<Mutex<Sink>> = Box::new(Mutex::new(Sink::default()));
    let client = std::ptr::from_ref(sink.as_ref())
        .cast::<c_void>()
        .cast_mut();

    let mut proc_id: AudioDeviceIOProcID = None;
    // SAFETY: `io_proc` matches the ABI; `client` stays valid until the guard
    // destroys the proc; `proc_id` is a valid out-pointer.
    let st = unsafe {
        AudioDeviceCreateIOProcID(device, Some(io_proc), client, NonNull::from(&mut proc_id))
    };
    if st != 0 || proc_id.is_none() {
        bail!("AudioDeviceCreateIOProcID failed (OSStatus {st})");
    }
    let proc = IoProc {
        device,
        id: proc_id,
    };

    // SAFETY: starting an IO proc we just registered on `device`.
    let st = unsafe { AudioDeviceStart(device, proc_id) };
    if st != 0 {
        bail!("AudioDeviceStart failed (OSStatus {st})");
    }
    std::thread::sleep(Duration::from_secs_f64(secs));
    drop(proc); // stop + destroy the proc before reading the sink (RT thread idle)

    let guard = sink.lock().map_err(|_| anyhow!("capture sink poisoned"))?;
    if guard.samples.is_empty() {
        bail!("the output tap produced no audio (nothing playing, or tap delivered no frames)");
    }
    // All-zero frames after a successful tap is the macOS signature of a missing
    // audio-capture (TCC) authorization — the tap runs but is muted. Surface the
    // cause instead of silently returning silence.
    if !guard.samples.iter().any(|&v| v != 0.0) {
        tracing::warn!(
            "host-output tap captured only silence — grant speak the macOS audio-capture \
             permission (run the signed bundle from `make app`, then allow the prompt; see \
             CLAUDE.md §4), or nothing is playing on the output device"
        );
    }
    Ok(PcmBuffer::new(
        guard.samples.clone(),
        rate,
        guard.channels.max(1),
    ))
}

/// Read a device's nominal sample rate (Hz).
unsafe fn nominal_sample_rate(device: AudioObjectID) -> Result<u32> {
    let mut addr = AudioObjectPropertyAddress {
        mSelector: kAudioDevicePropertyNominalSampleRate,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut rate: f64 = 0.0;
    let mut size = size_of::<f64>() as u32;
    // SAFETY: reads a single f64 sample rate from a valid device object.
    let st = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut addr),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut rate).cast(),
        )
    };
    if st != 0 || rate <= 0.0 {
        bail!("reading aggregate sample rate failed (OSStatus {st})");
    }
    Ok(rate as u32)
}

/// A running IO proc, stopped + destroyed on drop.
struct IoProc {
    device: AudioObjectID,
    id: AudioDeviceIOProcID,
}

impl Drop for IoProc {
    fn drop(&mut self) {
        // SAFETY: best-effort stop + destroy of a proc we registered.
        unsafe {
            let _ = AudioDeviceStop(self.device, self.id);
            let _ = AudioDeviceDestroyIOProcID(self.device, self.id);
        }
    }
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
        any(NSNumber::numberWithBool(true)), // private: read by id, not via the device list
        any(NSNumber::numberWithBool(true)), // tap auto-starts with the aggregate
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
