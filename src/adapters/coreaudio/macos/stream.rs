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
}

/// A started native capture bound to a ring (output tap or input engine),
/// stopped + torn down on drop.
pub(super) trait RingCapture {
    /// The ring the native callback fills.
    fn ring(&self) -> &CaptureRing;
}

/// Start a continuous capture; the receiver yields ~`chunk_secs` chunks until it
/// is dropped (which stops the capture). A start failure (permission /
/// unsupported) is surfaced synchronously.
pub(crate) fn start_capture_stream(
    source: &CaptureSource,
    chunk_secs: f64,
    cap_secs: f64,
) -> Result<mpsc::Receiver<PcmBuffer>> {
    let (tx, rx) = mpsc::channel::<PcmBuffer>(CHANNEL_CHUNKS);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    let direction = source.direction();
    let device = source.device();
    std::thread::Builder::new()
        .name("speak-capture".into())
        .spawn(move || producer(direction, device, chunk_secs, cap_secs, &tx, &ready_tx))
        .map_err(|e| anyhow!("spawning capture thread: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(rx),
        Ok(Err(e)) => Err(e),
        Err(_) => bail!("capture thread exited before signalling readiness"),
    }
}

/// Producer thread: own the native capture, drain `chunk_secs` chunks, and send
/// until the receiver is dropped (channel closed) — then the capture's Drop
/// stops it. The native capture never pauses between chunks (no gap, ADR-0017).
fn producer(
    direction: CaptureDirection,
    device: Option<u32>,
    chunk_secs: f64,
    cap_secs: f64,
    tx: &mpsc::Sender<PcmBuffer>,
    ready: &std::sync::mpsc::Sender<Result<()>>,
) {
    let capture: Box<dyn RingCapture> = match start(direction, device, cap_secs) {
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
    loop {
        let channels = ring.channels();
        if channels == 0 {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let want = (chunk_secs * f64::from(rate) * f64::from(channels)) as usize;
        match ring.drain(want.max(1)) {
            Some(samples) => {
                let chunk = PcmBuffer::new(samples, rate, channels);
                if tx.blocking_send(chunk).is_err() {
                    break; // receiver dropped → stop capturing
                }
            }
            None => std::thread::sleep(Duration::from_millis(POLL_MS)),
        }
    }
    // `capture` drops here → native capture stopped + destroyed.
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
