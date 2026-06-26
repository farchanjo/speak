//! libav (FFmpeg) codec layer.
//!
//! Responsibilities (codec role only, no device I/O, no process exec):
//!   * decode compressed network audio (mp3/opus/aac/flac/wav/pcm) into PCM,
//!     fed through a custom in-memory AVIO read callback;
//!   * resample PCM between formats via libswresample;
//!   * mux a minimal in-memory RIFF/WAVE buffer.
//!
//! All work goes through the linked `libav*` libraries via FFI; nothing is
//! shelled out.

use std::os::raw::{c_int, c_void};
use std::ptr;
use std::sync::Once;

use anyhow::{anyhow, bail, Context as _, Result};
use ffmpeg_the_third as ff;
use ff::ffi;

/// Canonical playback format the decoder resamples to. The native CoreAudio
/// mixer performs the final hardware-rate conversion.
pub const PLAY_RATE: u32 = 48_000;
/// Canonical playback channel count (stereo).
pub const PLAY_CHANNELS: u16 = 2;

/// Capture/ASR target format expected by the speech server.
pub const ASR_RATE: u32 = 16_000;
/// Capture/ASR channel count (mono).
pub const ASR_CHANNELS: u16 = 1;

/// Interleaved 32-bit float PCM with its sample rate and channel count.
#[derive(Debug, Clone)]
pub struct Pcm {
    /// Interleaved samples (`channels` values per frame).
    pub samples: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
}

impl Pcm {
    /// Number of audio frames (samples per channel).
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// Duration in seconds.
    #[must_use]
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate.max(1))
    }
}

fn ensure_init() -> Result<()> {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = ff::init();
        // Keep libav quiet on stderr (errors only).
        // SAFETY: setting the global log level is thread-safe in libavutil.
        unsafe { ffi::av_log_set_level(ffi::AV_LOG_ERROR) };
    });
    Ok(())
}

fn ff_err(code: c_int) -> anyhow::Error {
    anyhow!("libav error: {}", ff::Error::from(code))
}

/// Decode a compressed audio buffer into canonical 48 kHz stereo float PCM.
pub fn decode(bytes: Vec<u8>) -> Result<Pcm> {
    ensure_init()?;
    let mut avio = Avio::default();
    let mut input = open_mem_input(bytes, &mut avio)?;
    let samples = decode_stream(&mut input)?;
    drop(input);
    drop(avio);
    Ok(Pcm {
        samples,
        sample_rate: PLAY_RATE,
        channels: PLAY_CHANNELS,
    })
}

/// Resample interleaved float PCM to 16 kHz mono signed-16 for ASR upload.
pub fn to_asr_mono16(pcm: &Pcm) -> Result<Vec<i16>> {
    ensure_init()?;
    let mut in_layout = default_layout(i32::from(pcm.channels));
    // SAFETY: `in_layout` is a valid default layout that outlives this call.
    let mut resampler = unsafe {
        Resampler::new(
            ffi::AVSampleFormat::FLT,
            i32::from(pcm.channels),
            ptr::addr_of!(in_layout),
            pcm.sample_rate as i32,
            ffi::AVSampleFormat::S16,
            i32::from(ASR_CHANNELS),
            ASR_RATE as i32,
        )?
    };
    let frames = pcm.frames();
    // SAFETY: `planes[0]` points to `samples`, valid for `frames` frames.
    let mut bytes = unsafe {
        let planes = [pcm.samples.as_ptr().cast::<u8>()];
        resampler.convert(planes.as_ptr(), frames as c_int)?
    };
    bytes.extend(resampler.flush()?);
    drop(resampler);
    // SAFETY: avoids freeing the borrowed stack layout twice.
    unsafe { ffi::av_channel_layout_uninit(ptr::addr_of_mut!(in_layout)) };
    Ok(bytes_to_i16(&bytes))
}

/// Root-mean-square amplitude of signed-16 samples, normalised to `0.0..=1.0`.
#[must_use]
pub fn rms_s16(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| {
        let v = f64::from(s) / f64::from(i16::MAX);
        v * v
    }).sum();
    (sum / samples.len() as f64).sqrt()
}

/// Mux signed-16 mono PCM into an in-memory RIFF/WAVE buffer (no exec).
#[must_use]
pub fn wav_mono16(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut buf = Vec::with_capacity(44 + samples.len() * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

fn bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn default_layout(channels: i32) -> ffi::AVChannelLayout {
    // SAFETY: writes a fully-initialised default layout into the zeroed struct.
    unsafe {
        let mut layout = std::mem::zeroed::<ffi::AVChannelLayout>();
        ffi::av_channel_layout_default(ptr::addr_of_mut!(layout), channels);
        layout
    }
}

// ---------------------------------------------------------------------------
// Custom in-memory AVIO source
// ---------------------------------------------------------------------------

struct MemSource {
    data: Vec<u8>,
    pos: usize,
}

unsafe extern "C" fn read_packet(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
    let src = &mut *opaque.cast::<MemSource>();
    let remaining = src.data.len() - src.pos;
    if remaining == 0 {
        return ffi::AVERROR_EOF;
    }
    let n = remaining.min(buf_size as usize);
    ptr::copy_nonoverlapping(src.data.as_ptr().add(src.pos), buf, n);
    src.pos += n;
    n as c_int
}

/// Owns the raw AVIO context + opaque box and frees them after the wrapping
/// `Input` is dropped (declared before `Input`, so it drops last).
#[derive(Default)]
struct Avio {
    ctx: *mut ffi::AVIOContext,
    opaque: *mut MemSource,
}

impl Drop for Avio {
    fn drop(&mut self) {
        // SAFETY: the wrapping format context (CUSTOM_IO) never frees `pb`, so
        // this is the sole owner of the AVIO buffer, context and opaque box.
        unsafe {
            if !self.ctx.is_null() {
                ffi::av_freep(ptr::addr_of_mut!((*self.ctx).buffer).cast());
                ffi::avio_context_free(ptr::addr_of_mut!(self.ctx));
            }
            if !self.opaque.is_null() {
                drop(Box::from_raw(self.opaque));
            }
        }
    }
}

fn open_mem_input(bytes: Vec<u8>, avio: &mut Avio) -> Result<ff::format::context::Input> {
    const BUF: usize = 1 << 15;
    // SAFETY: each raw call is null-checked; ownership of buffer/opaque moves
    // into `avio` immediately so the Drop guard frees them on any early return.
    unsafe {
        let buffer = ffi::av_malloc(BUF).cast::<u8>();
        if buffer.is_null() {
            bail!("av_malloc failed");
        }
        let opaque = Box::into_raw(Box::new(MemSource { data: bytes, pos: 0 }));
        avio.opaque = opaque;
        let ctx = ffi::avio_alloc_context(
            buffer,
            BUF as c_int,
            0,
            opaque.cast(),
            Some(read_packet),
            None,
            None,
        );
        if ctx.is_null() {
            ffi::av_free(buffer.cast());
            bail!("avio_alloc_context failed");
        }
        avio.ctx = ctx;
        finish_open(ctx)
    }
}

unsafe fn finish_open(ctx: *mut ffi::AVIOContext) -> Result<ff::format::context::Input> {
    let mut fmt = ffi::avformat_alloc_context();
    if fmt.is_null() {
        bail!("avformat_alloc_context failed");
    }
    (*fmt).pb = ctx;
    (*fmt).flags |= ffi::AVFMT_FLAG_CUSTOM_IO;
    let rc = ffi::avformat_open_input(
        ptr::addr_of_mut!(fmt),
        ptr::null(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if rc < 0 {
        return Err(ff_err(rc)).context("avformat_open_input");
    }
    let rc = ffi::avformat_find_stream_info(fmt, ptr::null_mut());
    if rc < 0 {
        ffi::avformat_close_input(ptr::addr_of_mut!(fmt));
        return Err(ff_err(rc)).context("avformat_find_stream_info");
    }
    Ok(ff::format::context::Input::wrap(fmt))
}

// ---------------------------------------------------------------------------
// Decode loop
// ---------------------------------------------------------------------------

fn decode_stream(input: &mut ff::format::context::Input) -> Result<Vec<f32>> {
    let (index, mut decoder) = open_audio_decoder(input)?;
    let mut resampler: Option<Resampler> = None;
    let mut frame = ff::frame::Audio::empty();
    let mut out = Vec::<u8>::new();

    for item in input.packets() {
        let (stream, packet) = item?;
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet).context("send_packet")?;
        drain(&mut decoder, &mut frame, &mut resampler, &mut out)?;
    }
    decoder.send_eof().context("send_eof")?;
    drain(&mut decoder, &mut frame, &mut resampler, &mut out)?;
    if let Some(mut r) = resampler {
        out.extend(r.flush()?);
    }
    Ok(bytes_to_f32(&out))
}

fn open_audio_decoder(
    input: &mut ff::format::context::Input,
) -> Result<(usize, ff::codec::decoder::Audio)> {
    let stream = input
        .streams()
        .best(ff::media::Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream in server response"))?;
    let index = stream.index();
    let mut ctx = ff::codec::context::Context::from_parameters(stream.parameters())?;
    // Use all available local CPU cores where the codec supports threading.
    // (Audio codecs have no GPU/NVENC path; that hardware is server-side.)
    ctx.set_threading(ff::codec::threading::Config {
        kind: ff::codec::threading::Type::Frame,
        count: 0,
    });
    let decoder = ctx.decoder().audio().context("open audio decoder")?;
    Ok((index, decoder))
}

fn drain(
    decoder: &mut ff::codec::decoder::Audio,
    frame: &mut ff::frame::Audio,
    resampler: &mut Option<Resampler>,
    out: &mut Vec<u8>,
) -> Result<()> {
    while decoder.receive_frame(frame).is_ok() {
        let r = ensure_resampler(resampler, frame)?;
        // SAFETY: `frame` exposes valid `extended_data`/`nb_samples` for the
        // decoded format the resampler was configured against.
        let chunk = unsafe {
            let fp = frame.as_ptr();
            r.convert((*fp).extended_data.cast(), (*fp).nb_samples)?
        };
        out.extend(chunk);
    }
    Ok(())
}

fn ensure_resampler<'a>(
    resampler: &'a mut Option<Resampler>,
    frame: &ff::frame::Audio,
) -> Result<&'a mut Resampler> {
    if resampler.is_none() {
        // SAFETY: reads format fields from the freshly-decoded frame.
        let r = unsafe {
            let fp = frame.as_ptr();
            Resampler::new(
                ffi::AVSampleFormat((*fp).format),
                (*fp).ch_layout.nb_channels,
                ptr::addr_of!((*fp).ch_layout),
                (*fp).sample_rate,
                ffi::AVSampleFormat::FLT,
                i32::from(PLAY_CHANNELS),
                PLAY_RATE as i32,
            )?
        };
        *resampler = Some(r);
    }
    resampler
        .as_mut()
        .ok_or_else(|| anyhow!("resampler unavailable"))
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

// ---------------------------------------------------------------------------
// libswresample wrapper
// ---------------------------------------------------------------------------

struct Resampler {
    ctx: *mut ffi::SwrContext,
    out_fmt: ffi::AVSampleFormat,
    out_channels: i32,
    out_rate: i32,
    in_rate: i32,
}

impl Resampler {
    unsafe fn new(
        in_fmt: ffi::AVSampleFormat,
        in_channels: i32,
        in_layout: *const ffi::AVChannelLayout,
        in_rate: i32,
        out_fmt: ffi::AVSampleFormat,
        out_channels: i32,
        out_rate: i32,
    ) -> Result<Self> {
        let mut out_layout = default_layout(out_channels);
        let mut ctx: *mut ffi::SwrContext = ptr::null_mut();
        let rc = ffi::swr_alloc_set_opts2(
            ptr::addr_of_mut!(ctx),
            ptr::addr_of!(out_layout),
            out_fmt,
            out_rate,
            in_layout,
            in_fmt,
            in_rate,
            0,
            ptr::null_mut(),
        );
        ffi::av_channel_layout_uninit(ptr::addr_of_mut!(out_layout));
        if rc < 0 || ctx.is_null() {
            bail!("swr_alloc_set_opts2 failed");
        }
        let rc = ffi::swr_init(ctx);
        if rc < 0 {
            ffi::swr_free(ptr::addr_of_mut!(ctx));
            return Err(ff_err(rc)).context("swr_init");
        }
        let _ = in_channels;
        Ok(Self { ctx, out_fmt, out_channels, out_rate, in_rate })
    }

    fn bytes_per_sample(&self) -> usize {
        match self.out_fmt {
            ffi::AVSampleFormat::S16 => 2,
            _ => 4,
        }
    }

    unsafe fn run(&mut self, input: *const *const u8, in_samples: c_int) -> Result<Vec<u8>> {
        let delay = ffi::swr_get_delay(self.ctx, i64::from(self.in_rate.max(1)));
        let max_out = ffi::av_rescale_rnd(
            delay + i64::from(in_samples),
            i64::from(self.out_rate),
            i64::from(self.in_rate.max(1)),
            ffi::AVRounding::UP,
        )
        .max(0) as usize;
        let frame_bytes = self.out_channels as usize * self.bytes_per_sample();
        let mut buf = vec![0u8; max_out * frame_bytes];
        let planes = [buf.as_mut_ptr()];
        let got = ffi::swr_convert(self.ctx, planes.as_ptr(), max_out as c_int, input, in_samples);
        if got < 0 {
            return Err(ff_err(got)).context("swr_convert");
        }
        buf.truncate(got as usize * frame_bytes);
        Ok(buf)
    }

    /// Convert one block of interleaved/planar input samples.
    unsafe fn convert(&mut self, input: *const *const u8, in_samples: c_int) -> Result<Vec<u8>> {
        self.run(input, in_samples)
    }

    /// Flush any buffered samples after the final input block.
    fn flush(&mut self) -> Result<Vec<u8>> {
        // SAFETY: a null input with zero count drains the internal buffer.
        unsafe { self.run(ptr::null(), 0) }
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {
        // SAFETY: `ctx` was allocated by swr and is freed exactly once here.
        unsafe { ffi::swr_free(ptr::addr_of_mut!(self.ctx)) };
    }
}
