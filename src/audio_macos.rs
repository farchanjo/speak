//! Native macOS audio I/O via CoreAudio / AVFAudio (objc2 bindings).
//!
//! Output graph: `AVAudioPlayerNode` -> `AVAudioEngine.mainMixerNode` (the
//! native OS mixer) -> `outputNode`. Capture: `AVAudioEngine.inputNode` with
//! `installTapOnBus`. libav handles only codecs/resampling; every device I/O
//! and mix operation here is native CoreAudio. Nothing is shelled out.

use std::ptr::NonNull;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::AllocAnyThread;
use objc2_avf_audio::{
    AVAudioEngine, AVAudioFormat, AVAudioInputNode, AVAudioPCMBuffer, AVAudioPlayerNode,
    AVAudioPlayerNodeCompletionCallbackType, AVAudioTime,
};

use crate::codec::Pcm;

/// Play interleaved float PCM through the native CoreAudio mixer, blocking
/// until the buffer finishes rendering (or a safety timeout elapses).
pub fn play(pcm: &Pcm, volume: f32) -> Result<()> {
    if pcm.samples.is_empty() {
        bail!("no PCM samples to play");
    }
    let volume = volume.clamp(0.0, 1.0);
    autoreleasepool(|_| {
        objc2::exception::catch(|| play_inner(pcm, volume))
            .map_err(|e| anyhow!("CoreAudio playback raised an exception: {e:?}"))?
    })
}

/// Capture roughly `secs` seconds of microphone audio as interleaved float
/// PCM at the input device's native rate/channels. `device` is accepted for
/// API parity; only the system default input is wired today.
pub fn capture_chunk(device: u32, secs: f64) -> Result<Pcm> {
    let _ = device;
    autoreleasepool(|_| {
        objc2::exception::catch(|| capture_inner(secs))
            .map_err(|e| anyhow!("CoreAudio capture raised an exception: {e:?}"))?
    })
}

fn play_inner(pcm: &Pcm, volume: f32) -> Result<()> {
    // SAFETY: AVFAudio objects are created and wired on this thread following
    // the documented AVAudioEngine setup contract; pointers stay valid for the
    // duration of each call.
    unsafe {
        let format = make_format(pcm.sample_rate, pcm.channels)?;
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

unsafe fn make_format(rate: u32, channels: u16) -> Result<Retained<AVAudioFormat>> {
    // Standard format = deinterleaved float32, which AVAudioEngine connections
    // and AVAudioPlayerNode accept without raising format-mismatch exceptions.
    AVAudioFormat::initStandardFormatWithSampleRate_channels(
        AVAudioFormat::alloc(),
        f64::from(rate),
        u32::from(channels),
    )
    .ok_or_else(|| anyhow!("AVAudioFormat init failed"))
}

unsafe fn make_buffer(format: &AVAudioFormat, pcm: &Pcm) -> Result<Retained<AVAudioPCMBuffer>> {
    let channels = usize::from(pcm.channels.max(1));
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
            *plane.add(f) = pcm.samples[f * channels + c];
        }
    }
    Ok(buffer)
}

fn capture_inner(secs: f64) -> Result<Pcm> {
    // SAFETY: documented AVAudioEngine input-tap setup; the tap closure only
    // locks the shared buffer and is removed before the engine stops.
    unsafe {
        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
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
        Ok(Pcm {
            samples,
            sample_rate: rate as u32,
            channels: channels as u16,
        })
    }
}

unsafe fn install_tap(
    input: &AVAudioInputNode,
    format: &AVAudioFormat,
    store: &Arc<Mutex<Vec<f32>>>,
) {
    let sink = Arc::clone(store);
    let block = RcBlock::new(
        move |buf: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            // SAFETY: AVFAudio guarantees `buf` is valid for the callback.
            unsafe { append_buffer(buf.as_ref(), &sink) };
        },
    );
    // AVFAudio copies (retains) the block internally, so the local RcBlock may
    // drop after this call returns.
    input.installTapOnBus_bufferSize_format_block(0, 4096, Some(format), RcBlock::as_ptr(&block));
}

unsafe fn append_buffer(buf: &AVAudioPCMBuffer, sink: &Arc<Mutex<Vec<f32>>>) {
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

fn wait_capture(store: &Arc<Mutex<Vec<f32>>>, needed: usize, secs: f64) {
    let deadline = Instant::now() + Duration::from_secs_f64(secs + 2.0);
    loop {
        if store.lock().map(|g| g.len() >= needed).unwrap_or(true) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
