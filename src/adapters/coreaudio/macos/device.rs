//! CoreAudio HAL device enumeration (`kAudioHardwarePropertyDevices`, FR-10).
//!
//! Walks the HAL system object for every `AudioDeviceID`, then reads each
//! device's name, UID, per-direction channel counts, nominal sample rate, and
//! default-in/out status into the platform-neutral [`AudioDevice`] descriptor.
//! No CoreAudio type crosses the port boundary (ADR-0007).

use std::ptr::{self, NonNull};

use anyhow::{Result, bail};
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, kAudioDevicePropertyDeviceUID,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertyStreamConfiguration,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyElementMain, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};
use objc2_core_audio_types::AudioBufferList;
use objc2_core_foundation::{CFRetained, CFString};

use crate::ports::audio::{AudioDevice, AudioDeviceId};

const SYSTEM_OBJECT: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Enumerate every audio device with its direction, UID, rate and defaults.
pub fn enumerate() -> Result<Vec<AudioDevice>> {
    let default_in = default_device(kAudioHardwarePropertyDefaultInputDevice);
    let default_out = default_device(kAudioHardwarePropertyDefaultOutputDevice);
    let devices = device_ids()?
        .into_iter()
        .map(|id| describe(id, default_in, default_out))
        .collect();
    Ok(devices)
}

fn device_ids() -> Result<Vec<AudioObjectID>> {
    let mut addr = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut size: u32 = 0;
    // SAFETY: queries the system object's device-list byte size, then reads the
    // ids into a Vec sized to match; both raw calls are status-checked.
    unsafe {
        let st = AudioObjectGetPropertyDataSize(
            SYSTEM_OBJECT,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
        );
        if st != 0 {
            bail!("AudioObjectGetPropertyDataSize(devices) failed: OSStatus {st}");
        }
        let mut ids = vec![0u32; size as usize / size_of::<AudioObjectID>()];
        if ids.is_empty() {
            return Ok(ids);
        }
        let st = AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut *ids).cast(),
        );
        if st != 0 {
            bail!("AudioObjectGetPropertyData(devices) failed: OSStatus {st}");
        }
        ids.truncate(size as usize / size_of::<AudioObjectID>());
        Ok(ids)
    }
}

fn describe(id: AudioObjectID, default_in: Option<u32>, default_out: Option<u32>) -> AudioDevice {
    AudioDevice {
        id: AudioDeviceId(id),
        uid: cfstring_prop(id, kAudioDevicePropertyDeviceUID).unwrap_or_default(),
        name: cfstring_prop(id, kAudioObjectPropertyName).unwrap_or_else(|| format!("device {id}")),
        input_channels: channel_count(id, kAudioObjectPropertyScopeInput),
        output_channels: channel_count(id, kAudioObjectPropertyScopeOutput),
        sample_rate: nominal_rate(id),
        is_default_input: default_in == Some(id),
        is_default_output: default_out == Some(id),
    }
}

fn default_device(selector: u32) -> Option<AudioObjectID> {
    let mut addr = address(selector, kAudioObjectPropertyScopeGlobal);
    let mut id: AudioObjectID = 0;
    let mut size = size_of::<AudioObjectID>() as u32;
    // SAFETY: reads a single AudioObjectID from the system object.
    let st = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut id).cast(),
        )
    };
    (st == 0 && id != 0).then_some(id)
}

fn channel_count(id: AudioObjectID, scope: u32) -> u16 {
    let mut addr = address(kAudioDevicePropertyStreamConfiguration, scope);
    let mut size: u32 = 0;
    // SAFETY: reads the stream-config byte size, then the AudioBufferList into a
    // matching byte buffer; channel counts are summed across its buffers.
    unsafe {
        let st = AudioObjectGetPropertyDataSize(
            id,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
        );
        if st != 0 || size == 0 {
            return 0;
        }
        let mut buf = vec![0u8; size as usize];
        let st = AudioObjectGetPropertyData(
            id,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut *buf).cast(),
        );
        if st != 0 {
            return 0;
        }
        let list = &*buf.as_ptr().cast::<AudioBufferList>();
        let buffers =
            std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize);
        let total: u32 = buffers.iter().map(|b| b.mNumberChannels).sum();
        u16::try_from(total).unwrap_or(u16::MAX)
    }
}

fn nominal_rate(id: AudioObjectID) -> u32 {
    let mut addr = address(
        kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal,
    );
    let mut rate: f64 = 0.0;
    let mut size = size_of::<f64>() as u32;
    // SAFETY: reads a single Float64 nominal sample rate.
    let st = unsafe {
        AudioObjectGetPropertyData(
            id,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut rate).cast(),
        )
    };
    if st == 0 { rate.round() as u32 } else { 0 }
}

fn cfstring_prop(id: AudioObjectID, selector: u32) -> Option<String> {
    let mut addr = address(selector, kAudioObjectPropertyScopeGlobal);
    let mut cf: *const CFString = ptr::null();
    let mut size = size_of::<*const CFString>() as u32;
    // SAFETY: reads a +1 CFStringRef out-param; `CFRetained::from_raw` takes
    // ownership and releases it on drop.
    unsafe {
        let st = AudioObjectGetPropertyData(
            id,
            NonNull::from(&mut addr),
            0,
            ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut cf).cast(),
        );
        if st != 0 {
            return None;
        }
        let cf = CFRetained::from_raw(NonNull::new(cf.cast_mut())?);
        Some(cf.to_string())
    }
}
