//! Native macOS audio I/O via `CoreAudio` / `AVFAudio` (objc2 bindings).
//!
//! Output graph: `AVAudioPlayerNode` -> `AVAudioEngine.mainMixerNode` (the
//! native OS mixer) -> `outputNode`. Multi-output fan-out builds one
//! `AVAudioEngine` per target device, pinning each engine's output unit to a
//! specific `AudioDeviceID` via `kAudioOutputUnitProperty_CurrentDevice`, and
//! schedules the same decoded buffer on each (ADR-0007). Capture taps
//! `AVAudioEngine.inputNode`. libav owns codecs/resampling; every device I/O and
//! mix operation here is native `CoreAudio`. Nothing is shelled out.

use std::ptr::NonNull;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_audio_toolbox::{
    AudioUnit, AudioUnitSetProperty, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global,
};
use objc2_avf_audio::{
    AVAudioEngine, AVAudioFormat, AVAudioInputNode, AVAudioPCMBuffer, AVAudioPlayerNode,
    AVAudioPlayerNodeCompletionCallbackType, AVAudioTime,
};

use crate::domain::pcm::PcmBuffer;
use crate::ports::audio::AudioDeviceId;

/// Play interleaved float PCM through the native `CoreAudio` mixer on the default
/// output device, blocking until the buffer finishes (or a safety timeout).
pub fn play(pcm: &PcmBuffer, volume: f32) -> Result<()> {
    if pcm.is_empty() {
        bail!("no PCM samples to play");
    }
    let volume = volume.clamp(0.0, 1.0);
    autoreleasepool(|_| {
        objc2::exception::catch(|| play_inner(pcm, volume))
            .map_err(|e| anyhow!("CoreAudio playback raised an exception: {e:?}"))?
    })
}

/// Fan one decoded buffer out to every device in `devices` simultaneously.
///
/// Each device gets its own engine pinned to that `AudioDeviceID` (FR-11 / ADR-0007).
/// An empty list falls back to the default-device [`play`].
pub fn play_to(pcm: &PcmBuffer, devices: &[AudioDeviceId], volume: f32) -> Result<()> {
    if devices.is_empty() {
        return play(pcm, volume);
    }
    if pcm.is_empty() {
        bail!("no PCM samples to play");
    }
    let volume = volume.clamp(0.0, 1.0);
    autoreleasepool(|_| {
        objc2::exception::catch(|| play_to_inner(pcm, devices, volume))
            .map_err(|e| anyhow!("CoreAudio fan-out raised an exception: {e:?}"))?
    })
}

/// Capture roughly `secs` seconds of microphone audio as interleaved float PCM.
///
/// Audio is recorded at the input device's native rate and channel count.
/// A `Some(device)` pins the input unit to that `AudioDeviceID`; `None` uses
/// the system default input.
pub fn capture(device: Option<AudioDeviceId>, secs: f64) -> Result<PcmBuffer> {
    let device = device.map(|d| d.0);
    autoreleasepool(|_| {
        objc2::exception::catch(|| capture_inner(device, secs))
            .map_err(|e| anyhow!("CoreAudio capture raised an exception: {e:?}"))?
    })
}

/// Pin an output/input unit to a specific `AudioDeviceID` before the engine
/// starts (the AUHAL `CurrentDevice` property).
unsafe fn set_device(au: AudioUnit, device: u32) -> Result<()> {
    unsafe {
        let id = device;
        let st = AudioUnitSetProperty(
            au,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            0,
            std::ptr::addr_of!(id).cast(),
            size_of::<u32>() as u32,
        );
        if st != 0 {
            bail!("AudioUnitSetProperty(CurrentDevice={device}) failed: OSStatus {st}");
        }
        Ok(())
    }
}

fn play_inner(pcm: &PcmBuffer, volume: f32) -> Result<()> {
    // SAFETY: AVFAudio objects are created and wired on this thread following
    // the documented AVAudioEngine setup contract; pointers stay valid for the
    // duration of each call.
    unsafe {
        let format = make_format(pcm.sample_rate(), pcm.channels())?;
        let buffer = make_buffer(&format, pcm)?;
        let engine = AVAudioEngine::new();
        let player = AVAudioPlayerNode::new();
        engine.attachNode(&player);
        let mixer = engine.mainMixerNode();
        mixer.setOutputVolume(volume);
        engine.connect_to_format(&player, &mixer, Some(&format));
        engine.prepare();
        engine
            .startAndReturnError()
            .map_err(|e| anyhow!("AVAudioEngine start failed: {e:?}"))?;
        let (tx, rx) = sync_channel::<()>(1);
        let handler = RcBlock::new(move |_kind: AVAudioPlayerNodeCompletionCallbackType| {
            let _ = tx.try_send(());
        });
        player.scheduleBuffer_completionCallbackType_completionHandler(
            &buffer,
            AVAudioPlayerNodeCompletionCallbackType::DataPlayedBack,
            RcBlock::as_ptr(&handler),
        );
        player.play();
        let _ = rx.recv_timeout(Duration::from_secs_f64(pcm.duration_secs() + 1.0));
        player.stop();
        engine.stop();
    }
    Ok(())
}

/// A single running engine pinned to one device, kept alive for the fan-out.
struct Rig {
    engine: Retained<AVAudioEngine>,
    player: Retained<AVAudioPlayerNode>,
}

fn play_to_inner(pcm: &PcmBuffer, devices: &[AudioDeviceId], volume: f32) -> Result<()> {
    // SAFETY: one engine per device, each wired per the AVAudioEngine contract
    // and pinned to its `AudioDeviceID` before start; the rigs outlive playback.
    unsafe {
        let format = make_format(pcm.sample_rate(), pcm.channels())?;
        let mut rigs = Vec::with_capacity(devices.len());
        for dev in devices {
            rigs.push(build_rig(&format, pcm, volume, dev.0)?);
        }
        for rig in &rigs {
            rig.player.play();
        }
        std::thread::sleep(Duration::from_secs_f64(pcm.duration_secs() + 0.5));
        for rig in &rigs {
            rig.player.stop();
            rig.engine.stop();
        }
    }
    Ok(())
}

unsafe fn build_rig(
    format: &AVAudioFormat,
    pcm: &PcmBuffer,
    volume: f32,
    device: u32,
) -> Result<Rig> {
    unsafe {
        let buffer = make_buffer(format, pcm)?;
        let engine = AVAudioEngine::new();
        set_device(engine.outputNode().audioUnit(), device)?;
        let player = AVAudioPlayerNode::new();
        engine.attachNode(&player);
        let mixer = engine.mainMixerNode();
        mixer.setOutputVolume(volume);
        engine.connect_to_format(&player, &mixer, Some(format));
        engine.prepare();
        engine
            .startAndReturnError()
            .map_err(|e| anyhow!("AVAudioEngine start (device {device}) failed: {e:?}"))?;
        player.scheduleBuffer_completionCallbackType_completionHandler(
            &buffer,
            AVAudioPlayerNodeCompletionCallbackType::DataPlayedBack,
            std::ptr::null_mut(),
        );
        Ok(Rig { engine, player })
    }
}

unsafe fn make_format(rate: u32, channels: u16) -> Result<Retained<AVAudioFormat>> {
    unsafe {
        // Standard format = deinterleaved float32, which AVAudioEngine connections
        // and AVAudioPlayerNode accept without raising format-mismatch exceptions.
        AVAudioFormat::initStandardFormatWithSampleRate_channels(
            AVAudioFormat::alloc(),
            f64::from(rate),
            u32::from(channels),
        )
        .ok_or_else(|| anyhow!("AVAudioFormat init failed"))
    }
}

unsafe fn make_buffer(
    format: &AVAudioFormat,
    pcm: &PcmBuffer,
) -> Result<Retained<AVAudioPCMBuffer>> {
    unsafe {
        let channels = usize::from(pcm.channels().max(1));
        let frames = pcm.frames();
        let buffer = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            format,
            frames as u32,
        )
        .ok_or_else(|| anyhow!("AVAudioPCMBuffer init failed"))?;
        buffer.setFrameLength(frames as u32);
        let data = buffer.floatChannelData();
        if data.is_null() {
            bail!("AVAudioPCMBuffer exposed no float channel data");
        }
        // Deinterleave the interleaved source into one plane per channel.
        for c in 0..channels {
            let plane = (*data.add(c)).as_ptr();
            for f in 0..frames {
                *plane.add(f) = pcm.samples()[f * channels + c];
            }
        }
        Ok(buffer)
    }
}

fn capture_inner(device: Option<u32>, secs: f64) -> Result<PcmBuffer> {
    // SAFETY: documented AVAudioEngine input-tap setup; the tap closure only
    // locks the shared buffer and is removed before the engine stops.
    unsafe {
        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        if let Some(dev) = device {
            set_device(input.audioUnit(), dev)?;
        }
        let format = input.outputFormatForBus(0);
        let rate = format.sampleRate();
        let channels = usize::try_from(format.channelCount()).unwrap_or(0);
        if rate <= 0.0 || channels == 0 {
            bail!("microphone reported an invalid format (no input permission?)");
        }
        let store = Arc::new(Mutex::new(Vec::<f32>::new()));
        install_tap(&input, &format, &store);
        engine.prepare();
        engine
            .startAndReturnError()
            .map_err(|e| anyhow!("microphone engine start failed: {e:?}"))?;
        let needed = (secs * rate) as usize * channels;
        wait_capture(&store, needed, secs);
        input.removeTapOnBus(0);
        engine.stop();
        let samples = store
            .lock()
            .map_err(|_| anyhow!("capture buffer lock poisoned"))?
            .clone();
        Ok(PcmBuffer::new(samples, rate as u32, channels as u16))
    }
}

unsafe fn install_tap(
    input: &AVAudioInputNode,
    format: &AVAudioFormat,
    store: &Arc<Mutex<Vec<f32>>>,
) {
    unsafe {
        let sink = Arc::clone(store);
        let block = RcBlock::new(
            move |buf: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                // SAFETY: AVFAudio guarantees `buf` is valid for the callback.
                append_buffer(buf.as_ref(), &sink);
            },
        );
        // AVFAudio copies (retains) the block internally, so the local RcBlock may
        // drop after this call returns.
        input.installTapOnBus_bufferSize_format_block(
            0,
            4096,
            Some(format),
            RcBlock::as_ptr(&block),
        );
    }
}

unsafe fn append_buffer(buf: &AVAudioPCMBuffer, sink: &Arc<Mutex<Vec<f32>>>) {
    unsafe {
        let frames = buf.frameLength() as usize;
        if frames == 0 {
            return;
        }
        let format = buf.format();
        let channels = usize::try_from(format.channelCount()).unwrap_or(0);
        let data = buf.floatChannelData();
        if channels == 0 || data.is_null() {
            return;
        }
        let Ok(mut guard) = sink.lock() else {
            return;
        };
        if format.isInterleaved() {
            let slice = std::slice::from_raw_parts((*data).as_ptr(), frames * channels);
            guard.extend_from_slice(slice);
        } else {
            for f in 0..frames {
                for c in 0..channels {
                    let plane = (*data.add(c)).as_ptr();
                    guard.push(*plane.add(f));
                }
            }
        }
    }
}

fn wait_capture(store: &Arc<Mutex<Vec<f32>>>, needed: usize, secs: f64) {
    let deadline = Instant::now() + Duration::from_secs_f64(secs + 2.0);
    loop {
        if store.lock().map_or(true, |g| g.len() >= needed) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
