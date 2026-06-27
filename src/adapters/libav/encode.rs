//! Record-output encoders for the `libav` adapter (T038 / FR-9).
//!
//! Two directions, both fully in-process and exec-free (ADR-0001):
//!   * `encode_wav` hand-muxes a RIFF/WAVE file (no encoder), and
//!   * `encode_flac` drives the libavcodec FLAC encoder, muxing the `.flac`
//!     container through a custom in-memory AVIO **write** callback — the mirror
//!     of the decode read callback. No temp files, no child process.
//!
//! Captured PCM ([`crate::domain::pcm::PcmBuffer`], interleaved f32 at the mic's
//! native rate/channels) is quantised to signed-16 first; FLAC is lossless over
//! that s16 stream.

use std::os::raw::{c_int, c_void};
use std::ptr;

use anyhow::{Context as _, Result, bail};
use ff::ffi;
use ffmpeg_the_third as ff;

use super::codec::{self, f32_to_i16, wav_pcm16};
use crate::domain::pcm::PcmBuffer;

/// Encode captured PCM into a hand-muxed RIFF/WAVE file (no encoder, no exec).
#[must_use]
pub fn encode_wav(pcm: &PcmBuffer) -> Vec<u8> {
    let s16 = f32_to_i16(pcm.samples());
    wav_pcm16(&s16, pcm.sample_rate(), pcm.channels())
}

/// Encode captured PCM into FLAC via the libavcodec FLAC encoder through a
/// custom in-memory AVIO write callback (no temp files, no exec).
pub fn encode_flac(pcm: &PcmBuffer) -> Result<Vec<u8>> {
    codec::ensure_init()?;
    let s16 = f32_to_i16(pcm.samples());
    let mut enc = FlacEncoder::open(pcm.sample_rate().max(1), pcm.channels().max(1))?;
    enc.write_samples(&s16)?;
    enc.finish()
}

// ---------------------------------------------------------------------------
// Custom in-memory AVIO write sink
// ---------------------------------------------------------------------------

/// Growable in-memory sink behind the output AVIO context (the FLAC muxer seeks
/// back to patch the STREAMINFO header, so the sink supports random access).
struct MemSink {
    data: Vec<u8>,
    pos: usize,
}

unsafe extern "C" fn write_packet(opaque: *mut c_void, buf: *const u8, size: c_int) -> c_int {
    unsafe {
        let sink = &mut *opaque.cast::<MemSink>();
        let bytes = std::slice::from_raw_parts(buf, size as usize);
        let end = sink.pos + bytes.len();
        if end > sink.data.len() {
            sink.data.resize(end, 0);
        }
        sink.data[sink.pos..end].copy_from_slice(bytes);
        sink.pos = end;
        size
    }
}

unsafe extern "C" fn seek_sink(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
    unsafe {
        let sink = &mut *opaque.cast::<MemSink>();
        if whence & ffi::AVSEEK_SIZE != 0 {
            return sink.data.len() as i64;
        }
        let base = whence & !(ffi::AVSEEK_SIZE | ffi::AVSEEK_FORCE);
        let new = match base {
            0 => offset,                          // SEEK_SET
            1 => sink.pos as i64 + offset,        // SEEK_CUR
            2 => sink.data.len() as i64 + offset, // SEEK_END
            _ => return -1,
        };
        if new < 0 {
            return -1;
        }
        sink.pos = new as usize;
        new
    }
}

// ---------------------------------------------------------------------------
// FLAC encoder + muxer (owns every raw pointer; Drop frees them all)
// ---------------------------------------------------------------------------

struct FlacEncoder {
    cctx: *mut ffi::AVCodecContext,
    ofmt: *mut ffi::AVFormatContext,
    avio: *mut ffi::AVIOContext,
    sink: *mut MemSink,
    frame: *mut ffi::AVFrame,
    pkt: *mut ffi::AVPacket,
    stream_index: c_int,
    src_tb: ffi::AVRational,
    dst_tb: ffi::AVRational,
    channels: u16,
    frame_size: usize,
    pts: i64,
}

impl FlacEncoder {
    fn open(rate: u32, channels: u16) -> Result<Self> {
        let tb = ffi::AVRational {
            num: 1,
            den: rate as c_int,
        };
        let mut enc = Self {
            cctx: ptr::null_mut(),
            ofmt: ptr::null_mut(),
            avio: ptr::null_mut(),
            sink: ptr::null_mut(),
            frame: ptr::null_mut(),
            pkt: ptr::null_mut(),
            stream_index: 0,
            src_tb: tb,
            dst_tb: tb,
            channels,
            frame_size: 0,
            pts: 0,
        };
        // SAFETY: each step null-checks its allocation; on error `enc` drops,
        // freeing whatever was built so far.
        unsafe {
            enc.open_codec(rate, channels)?;
            enc.open_muxer(rate)?;
            enc.alloc_frame_pkt(rate, channels)?;
        }
        Ok(enc)
    }

    unsafe fn open_codec(&mut self, rate: u32, channels: u16) -> Result<()> {
        unsafe {
            let codec = ffi::avcodec_find_encoder(ffi::AVCodecID::FLAC);
            if codec.is_null() {
                bail!("FLAC encoder not available in this libav build");
            }
            let cctx = ffi::avcodec_alloc_context3(codec);
            if cctx.is_null() {
                bail!("avcodec_alloc_context3 failed");
            }
            self.cctx = cctx;
            (*cctx).sample_fmt = ffi::AVSampleFormat::S16;
            (*cctx).sample_rate = rate as c_int;
            (*cctx).time_base = self.src_tb;
            ffi::av_channel_layout_default(
                ptr::addr_of_mut!((*cctx).ch_layout),
                i32::from(channels),
            );
            let rc = ffi::avcodec_open2(cctx, codec, ptr::null_mut());
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("avcodec_open2 (flac)");
            }
            let fs = (*cctx).frame_size;
            self.frame_size = if fs > 0 { fs as usize } else { 4096 };
            Ok(())
        }
    }

    unsafe fn open_muxer(&mut self, rate: u32) -> Result<()> {
        unsafe {
            let mut ofmt: *mut ffi::AVFormatContext = ptr::null_mut();
            let rc = ffi::avformat_alloc_output_context2(
                ptr::addr_of_mut!(ofmt),
                ptr::null(),
                c"flac".as_ptr(),
                ptr::null(),
            );
            if rc < 0 || ofmt.is_null() {
                return Err(codec::ff_err(rc)).context("alloc flac output context");
            }
            self.ofmt = ofmt;
            self.alloc_io()?;
            (*ofmt).pb = self.avio;
            (*ofmt).flags |= ffi::AVFMT_FLAG_CUSTOM_IO;
            let st = ffi::avformat_new_stream(ofmt, ptr::null());
            if st.is_null() {
                bail!("avformat_new_stream failed");
            }
            let rc = ffi::avcodec_parameters_from_context((*st).codecpar, self.cctx);
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("avcodec_parameters_from_context");
            }
            (*st).time_base = self.src_tb;
            self.stream_index = (*st).index;
            self.dst_tb = (*st).time_base;
            let _ = rate;
            let rc = ffi::avformat_write_header(ofmt, ptr::null_mut());
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("avformat_write_header (flac)");
            }
            Ok(())
        }
    }

    unsafe fn alloc_io(&mut self) -> Result<()> {
        const BUF: usize = 1 << 12;
        unsafe {
            let buffer = ffi::av_malloc(BUF).cast::<u8>();
            if buffer.is_null() {
                bail!("av_malloc failed");
            }
            let sink = Box::into_raw(Box::new(MemSink {
                data: Vec::new(),
                pos: 0,
            }));
            self.sink = sink;
            let avio = ffi::avio_alloc_context(
                buffer,
                BUF as c_int,
                1,
                sink.cast(),
                None,
                Some(write_packet),
                Some(seek_sink),
            );
            if avio.is_null() {
                ffi::av_free(buffer.cast());
                bail!("avio_alloc_context failed");
            }
            self.avio = avio;
            Ok(())
        }
    }

    unsafe fn alloc_frame_pkt(&mut self, rate: u32, channels: u16) -> Result<()> {
        unsafe {
            let frame = ffi::av_frame_alloc();
            let pkt = ffi::av_packet_alloc();
            if frame.is_null() || pkt.is_null() {
                bail!("frame/packet allocation failed");
            }
            self.frame = frame;
            self.pkt = pkt;
            (*frame).format = ffi::AVSampleFormat::S16.0;
            (*frame).sample_rate = rate as c_int;
            (*frame).nb_samples = self.frame_size as c_int;
            ffi::av_channel_layout_default(
                ptr::addr_of_mut!((*frame).ch_layout),
                i32::from(channels),
            );
            let rc = ffi::av_frame_get_buffer(frame, 0);
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("av_frame_get_buffer");
            }
            Ok(())
        }
    }

    /// Feed interleaved s16 samples through the encoder in frame-sized blocks.
    fn write_samples(&mut self, s16: &[i16]) -> Result<()> {
        let ch = usize::from(self.channels.max(1));
        let block = self.frame_size.max(1) * ch;
        for chunk in s16.chunks(block) {
            self.send_chunk(chunk, ch)?;
        }
        Ok(())
    }

    fn send_chunk(&mut self, chunk: &[i16], ch: usize) -> Result<()> {
        let frames = chunk.len() / ch;
        if frames == 0 {
            return Ok(());
        }
        // SAFETY: the frame buffer holds `frame_size*ch*2` bytes >= chunk bytes;
        // `data[0]` is the packed s16 plane for AV_SAMPLE_FMT_S16.
        unsafe {
            let rc = ffi::av_frame_make_writable(self.frame);
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("av_frame_make_writable");
            }
            (*self.frame).nb_samples = frames as c_int;
            ptr::copy_nonoverlapping(
                chunk.as_ptr().cast::<u8>(),
                (*self.frame).data[0],
                chunk.len() * 2,
            );
            (*self.frame).pts = self.pts;
            self.pts += frames as i64;
            self.encode(self.frame)
        }
    }

    unsafe fn encode(&mut self, frame: *mut ffi::AVFrame) -> Result<()> {
        unsafe {
            let rc = ffi::avcodec_send_frame(self.cctx, frame);
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("avcodec_send_frame (flac)");
            }
            self.drain()
        }
    }

    unsafe fn drain(&mut self) -> Result<()> {
        unsafe {
            loop {
                let rc = ffi::avcodec_receive_packet(self.cctx, self.pkt);
                if rc < 0 {
                    // EAGAIN (need more input) or EOF (flushed): stop draining.
                    return Ok(());
                }
                (*self.pkt).stream_index = self.stream_index;
                ffi::av_packet_rescale_ts(self.pkt, self.src_tb, self.dst_tb);
                let wr = ffi::av_interleaved_write_frame(self.ofmt, self.pkt);
                ffi::av_packet_unref(self.pkt);
                if wr < 0 {
                    return Err(codec::ff_err(wr)).context("av_interleaved_write_frame");
                }
            }
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>> {
        // SAFETY: a null frame flushes the encoder; the trailer patches the
        // FLAC header via the seekable sink; `sink` is a live boxed MemSink.
        unsafe {
            let rc = ffi::avcodec_send_frame(self.cctx, ptr::null());
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("flush avcodec_send_frame");
            }
            self.drain()?;
            let rc = ffi::av_write_trailer(self.ofmt);
            if rc < 0 {
                return Err(codec::ff_err(rc)).context("av_write_trailer (flac)");
            }
            Ok((*self.sink).data.clone())
        }
    }
}

impl Drop for FlacEncoder {
    fn drop(&mut self) {
        // SAFETY: each pointer is freed exactly once; nulls are skipped. The
        // muxer holds CUSTOM_IO so it never frees `pb`; the AVIO buffer, context
        // and boxed sink are freed here.
        unsafe {
            if !self.frame.is_null() {
                ffi::av_frame_free(ptr::addr_of_mut!(self.frame));
            }
            if !self.pkt.is_null() {
                ffi::av_packet_free(ptr::addr_of_mut!(self.pkt));
            }
            if !self.cctx.is_null() {
                ffi::avcodec_free_context(ptr::addr_of_mut!(self.cctx));
            }
            if !self.ofmt.is_null() {
                ffi::avformat_free_context(self.ofmt);
                self.ofmt = ptr::null_mut();
            }
            if !self.avio.is_null() {
                ffi::av_freep(ptr::addr_of_mut!((*self.avio).buffer).cast());
                ffi::avio_context_free(ptr::addr_of_mut!(self.avio));
            }
            if !self.sink.is_null() {
                drop(Box::from_raw(self.sink));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::libav::codec::{DecodeOptions, decode};

    fn sine(rate: u32, channels: u16, secs: f64) -> PcmBuffer {
        let frames = (f64::from(rate) * secs) as usize;
        let mut samples = Vec::with_capacity(frames * usize::from(channels));
        for f in 0..frames {
            let t = f as f64 / f64::from(rate);
            let v = (t * 440.0 * std::f64::consts::TAU).sin() as f32 * 0.5;
            for _ in 0..channels {
                samples.push(v);
            }
        }
        PcmBuffer::new(samples, rate, channels)
    }

    #[test]
    fn wav_encode_is_riff_with_channel_header() {
        let pcm = sine(16_000, 2, 0.05);
        let wav = encode_wav(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2); // channels
    }

    #[test]
    fn flac_encode_emits_magic_and_round_trips() {
        let pcm = sine(16_000, 1, 0.25);
        let flac = encode_flac(&pcm).expect("flac encode");
        assert!(flac.len() > 4, "flac output too small");
        assert_eq!(&flac[0..4], b"fLaC", "missing FLAC stream marker");
        // Round-trip back through the decoder: a valid container decodes to
        // ~0.25 s of 48 kHz stereo playback PCM.
        let back = decode(flac, &DecodeOptions::default()).expect("decode flac");
        assert!(
            back.duration_secs() > 0.2,
            "decoded {}",
            back.duration_secs()
        );
    }

    #[test]
    fn flac_encode_handles_stereo() {
        let pcm = sine(44_100, 2, 0.1);
        let flac = encode_flac(&pcm).expect("stereo flac encode");
        assert_eq!(&flac[0..4], b"fLaC");
    }
}
