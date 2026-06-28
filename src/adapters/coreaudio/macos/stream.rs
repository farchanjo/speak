//! Continuous streaming capture (ADR-0017).
//!
//! One long-lived native capture per session — the output tap's
//! `AudioDeviceIOProc` ([`tap`]) or the `AVAudioEngine` input tap ([`engine`]) —
//! fills a bounded [`CaptureRing`] continuously; a dedicated producer thread
//! slices it into `chunk_secs`-sized [`PcmBuffer`]s and sends them over a bounded
//! channel. This decouples capture from the SSE consumer so a slow round trip
//! never stalls capture (no dropped words). Hybrid backpressure: the bounded
//! channel pushes back on the producer, and the ring drops the oldest frames
//! only past `cap_secs`.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::sync::mpsc;

use crate::adapters::coreaudio::SegmentParams;
use crate::domain::capture_source::{CaptureDirection, CaptureSource};
use crate::domain::pcm::PcmBuffer;

use super::{engine, tap};

/// How many chunk slots the producer→consumer channel buffers before the
/// producer blocks (then the ring's `cap_secs` ceiling governs drop-oldest).
/// Kept shallow (ADR-0018): the pipelined consumer overlaps round trips, so a
/// deep channel would only add hidden latency — the ring (`buffer_secs`) is the
/// real backpressure ceiling.
const CHANNEL_CHUNKS: usize = 2;
/// Poll interval while the producer waits for a full chunk to accumulate.
const POLL_MS: u64 = 20;

// VAD segmentation (ADR-0019): cut on the silence between utterances instead of
// a fixed time grid, so a word straddling a boundary is never split and a quiet
// line is never dropped wholesale.
/// Shortest speech content kept as a segment; anything briefer is treated as a
/// noise blip and dropped (avoids one-word fragments from coughs/clicks).
const MIN_SPEECH_SECS: f64 = 0.25;
/// Trailing silence that closes the current segment (a natural sentence pause).
/// 0.7 s rides over dramatic mid-sentence pauses so a clause is not split, while
/// still flushing promptly between lines (standard VAD hangover range).
const HANG_SECS: f64 = 0.7;
/// Hard cap so unbroken speech still flushes (one cut, in the rare no-pause case).
const MAX_SEGMENT_SECS: f64 = 14.0;
/// Audio retained before speech onset so the leading consonant is not clipped.
const PRE_ROLL_SECS: f64 = 0.3;
/// Audio retained after speech before trailing silence is trimmed.
const POST_ROLL_SECS: f64 = 0.3;

/// Bounded interleaved-float ring the native capture callback fills (ADR-0017).
///
/// The real-time callback ([`push`](Self::push)) drops the oldest frames once
/// the backlog passes `cap_secs` — the hybrid backpressure ceiling.
pub(super) struct CaptureRing {
    inner: Mutex<VecDeque<f32>>,
    rate: AtomicU32,
    channels: AtomicU16,
    cap_secs: f64,
}

impl CaptureRing {
    /// Build a ring at `rate` Hz bounded to `cap_secs` of audio.
    pub(super) fn new(rate: u32, cap_secs: f64) -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            rate: AtomicU32::new(rate),
            channels: AtomicU16::new(0),
            cap_secs,
        }
    }

    /// Append interleaved samples from the capture callback, dropping the oldest
    /// frames past the `cap_secs` ceiling. Records the channel count.
    pub(super) fn push(&self, data: &[f32], channels: u16) {
        if channels == 0 {
            return;
        }
        self.channels.store(channels, Ordering::Relaxed);
        let rate = f64::from(self.rate.load(Ordering::Relaxed).max(1));
        let cap = (self.cap_secs * rate * f64::from(channels)) as usize;
        let Ok(mut queue) = self.inner.lock() else {
            return;
        };
        queue.extend(data.iter().copied());
        let overflow = queue.len().saturating_sub(cap.max(1));
        if overflow > 0 {
            drop(queue.drain(..overflow));
        }
    }

    fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    fn channels(&self) -> u16 {
        self.channels.load(Ordering::Relaxed)
    }

    /// Drain exactly `want` interleaved samples, or `None` if not yet available.
    fn drain(&self, want: usize) -> Option<Vec<f32>> {
        let mut queue = self.inner.lock().ok()?;
        (queue.len() >= want).then(|| queue.drain(..want).collect())
    }

    /// Drain everything currently buffered (the VAD segmenter's incremental hop),
    /// or `None` when empty.
    fn drain_all(&self) -> Option<Vec<f32>> {
        let mut queue = self.inner.lock().ok()?;
        (!queue.is_empty()).then(|| queue.drain(..).collect())
    }
}

/// Root-mean-square amplitude of an interleaved float slice (VAD energy).
fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// Silence-boundary segmenter (ADR-0019). Fed incremental interleaved hops, it
/// accumulates a segment and emits it on a trailing-silence pause (or a hard cap),
/// dropping pure-noise blips. State is in interleaved-sample counts.
struct Segmenter {
    rate: u32,
    channels: u16,
    floor: f64,
    min: usize,
    hang: usize,
    max: usize,
    preroll: usize,
    postroll: usize,
    buf: Vec<f32>,
    seen_speech: bool,
    speech_samples: usize,
    silence_run: usize,
}

impl Segmenter {
    fn new(rate: u32, channels: u16, floor: f64) -> Self {
        let inter = |secs: f64| (secs * f64::from(rate) * f64::from(channels)) as usize;
        Self {
            rate,
            channels,
            floor,
            min: inter(MIN_SPEECH_SECS).max(1),
            hang: inter(HANG_SECS).max(1),
            max: inter(MAX_SEGMENT_SECS).max(1),
            preroll: inter(PRE_ROLL_SECS),
            postroll: inter(POST_ROLL_SECS),
            buf: Vec::new(),
            seen_speech: false,
            speech_samples: 0,
            silence_run: 0,
        }
    }

    /// Feed one hop; returns a completed segment when a pause (or the cap) closes it.
    fn push(&mut self, hop: &[f32]) -> Option<PcmBuffer> {
        if hop.is_empty() {
            return None;
        }
        let loud = rms(hop) >= self.floor;
        self.buf.extend_from_slice(hop);
        if loud {
            self.seen_speech = true;
            self.speech_samples = self.speech_samples.saturating_add(hop.len());
            self.silence_run = 0;
        } else if self.seen_speech {
            self.silence_run = self.silence_run.saturating_add(hop.len());
        } else if self.buf.len() > self.preroll {
            // Leading silence: keep only the pre-roll tail so speech starts ~PRE_ROLL_SECS in.
            let drop = self.buf.len() - self.preroll;
            self.buf.drain(..drop);
        }
        if self.buf.len() >= self.max {
            return self.flush();
        }
        if self.seen_speech && self.silence_run >= self.hang {
            return self.flush();
        }
        None
    }

    /// Close the segment: emit trimmed speech (≥ `min`), or drop it as noise. Resets.
    fn flush(&mut self) -> Option<PcmBuffer> {
        let speech_end = self.buf.len().saturating_sub(self.silence_run);
        let out = if self.seen_speech && self.speech_samples >= self.min {
            let keep = speech_end.saturating_add(self.postroll).min(self.buf.len());
            Some(PcmBuffer::new(
                self.buf[..keep].to_vec(),
                self.rate,
                self.channels,
            ))
        } else {
            None
        };
        self.buf.clear();
        self.seen_speech = false;
        self.speech_samples = 0;
        self.silence_run = 0;
        out
    }
}

/// A started native capture bound to a ring (output tap or input engine),
/// stopped + torn down on drop.
pub(super) trait RingCapture {
    /// The ring the native callback fills.
    fn ring(&self) -> &CaptureRing;
}

/// Start a continuous capture; the receiver yields VAD-segmented utterances (or
/// fixed `chunk_secs` slices when `params.vad` is off) until it is dropped (which
/// stops the capture). A start failure (permission / unsupported) is surfaced
/// synchronously.
pub(crate) fn start_capture_stream(
    source: &CaptureSource,
    params: SegmentParams,
) -> Result<mpsc::Receiver<PcmBuffer>> {
    let (tx, rx) = mpsc::channel::<PcmBuffer>(CHANNEL_CHUNKS);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    let direction = source.direction();
    let device = source.device();
    std::thread::Builder::new()
        .name("speak-capture".into())
        .spawn(move || producer(direction, device, params, &tx, &ready_tx))
        .map_err(|e| anyhow!("spawning capture thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(rx),
        Ok(Err(e)) => Err(e),
        Err(_) => bail!("capture thread exited before signalling readiness"),
    }
}

/// Producer thread: own the native capture and send segments until the receiver
/// is dropped (channel closed) — then the capture's Drop stops it. The native
/// capture never pauses between segments (no gap, ADR-0017). With VAD on it cuts
/// on the silence between utterances (ADR-0019); off, it slices a fixed grid.
fn producer(
    direction: CaptureDirection,
    device: Option<u32>,
    params: SegmentParams,
    tx: &mpsc::Sender<PcmBuffer>,
    ready: &std::sync::mpsc::Sender<Result<()>>,
) {
    let capture: Box<dyn RingCapture> = match start(direction, device, params.cap_secs) {
        Ok(c) => {
            let _ = ready.send(Ok(()));
            c
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let ring = capture.ring();
    let rate = ring.rate();
    let channels = wait_for_channels(ring);
    if params.vad {
        let mut seg = Segmenter::new(rate, channels, params.floor);
        loop {
            match ring.drain_all() {
                Some(hop) => {
                    if let Some(segment) = seg.push(&hop)
                        && tx.blocking_send(segment).is_err()
                    {
                        break; // receiver dropped → stop capturing
                    }
                }
                None => std::thread::sleep(Duration::from_millis(POLL_MS)),
            }
        }
    } else {
        let want = ((params.chunk_secs * f64::from(rate) * f64::from(channels)) as usize).max(1);
        loop {
            match ring.drain(want) {
                Some(samples) => {
                    let chunk = PcmBuffer::new(samples, rate, channels);
                    if tx.blocking_send(chunk).is_err() {
                        break;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(POLL_MS)),
            }
        }
    }
    // `capture` drops here → native capture stopped + destroyed.
}

/// Block until the native callback reports a non-zero channel count (the first
/// buffer has arrived), so the segmenter can size its windows.
fn wait_for_channels(ring: &CaptureRing) -> u16 {
    loop {
        let channels = ring.channels();
        if channels != 0 {
            return channels;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Build the native capture for the requested direction.
fn start(
    direction: CaptureDirection,
    device: Option<u32>,
    cap_secs: f64,
) -> Result<Box<dyn RingCapture>> {
    match direction {
        CaptureDirection::Output => Ok(Box::new(tap::start_output_capture(device, cap_secs)?)),
        CaptureDirection::Input => Ok(Box::new(engine::start_input_capture(device, cap_secs)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::{HANG_SECS, MIN_SPEECH_SECS, Segmenter};

    const RATE: u32 = 16_000;
    const FLOOR: f64 = 0.1;

    fn seg() -> Segmenter {
        Segmenter::new(RATE, 1, FLOOR)
    }

    fn hop(samples: usize, amp: f32) -> Vec<f32> {
        vec![amp; samples]
    }

    fn secs(n: f64) -> usize {
        (n * f64::from(RATE)) as usize
    }

    #[test]
    fn emits_one_trimmed_segment_on_a_silence_pause() {
        let mut s = seg();
        // 0.5s speech → below cap, no pause yet.
        assert!(s.push(&hop(secs(0.5), 0.5)).is_none());
        // HANG_SECS of silence closes it.
        let out = s
            .push(&hop(secs(HANG_SECS), 0.0))
            .expect("pause flushes the utterance");
        assert_eq!(out.sample_rate(), RATE);
        assert_eq!(out.channels(), 1);
        // Trailing silence is trimmed to the post-roll, not the full hang window.
        assert!(out.samples().len() < secs(0.5) + secs(HANG_SECS));
        assert!(out.samples().len() >= secs(0.5));
    }

    #[test]
    fn drops_a_short_noise_blip() {
        let mut s = seg();
        // Speech shorter than MIN_SPEECH_SECS → treated as noise.
        assert!(s.push(&hop(secs(MIN_SPEECH_SECS / 2.0), 0.5)).is_none());
        assert!(
            s.push(&hop(secs(HANG_SECS), 0.0)).is_none(),
            "a sub-minimum blip is dropped, not emitted"
        );
    }

    #[test]
    fn a_short_mid_sentence_pause_does_not_split() {
        let mut s = seg();
        assert!(s.push(&hop(secs(0.5), 0.5)).is_none());
        // Brief gap (< HANG_SECS) must not close the segment.
        assert!(s.push(&hop(secs(HANG_SECS / 2.0), 0.0)).is_none());
        assert!(s.push(&hop(secs(0.5), 0.5)).is_none());
        let out = s
            .push(&hop(secs(HANG_SECS), 0.0))
            .expect("the real pause flushes one combined segment");
        // Both speech bursts + the inner gap are in a single segment.
        assert!(out.samples().len() > secs(1.0));
    }

    #[test]
    fn caps_unbroken_speech() {
        let mut s = seg();
        // Speech past MAX_SEGMENT_SECS with no pause must still flush (hard cap).
        let out = s
            .push(&hop(secs(15.0), 0.5))
            .expect("the cap flushes unbroken speech");
        assert!(!out.samples().is_empty());
    }
}
