//! Pipelined in-flight SSE consumer for streaming capture (ADR-0018).
//!
//! ADR-0017 decoupled native capture from the consumer with a bounded channel so
//! capture never stalls. This module fixes the *other* half: the consumer was
//! serial (one chunk `POSTed` and drained to `done` before the next was received),
//! so a round trip ≈ `chunk_secs` made it fall behind realtime until the ring
//! dropped audio. Here up to [`MAX_INFLIGHT`] chunk round trips overlap, and a
//! [`FuturesOrdered`] presents completed chunks in **capture order** — output is
//! never interleaved even though round trips finish out of order.
//!
//! Shared by `transcribe --stream` and `translate --stream`; `realtime` stays
//! serial (it plays audio back, so out-of-order playback would be wrong).

use std::future::Future;

use anyhow::Result;
use futures_util::stream::{FuturesOrdered, StreamExt};

use speak::adapters::config::Config;
use speak::adapters::coreaudio::NativeCaptureStream;
use speak::adapters::sse::{RealtimeRequest, SseRealtimeClient};
use speak::application::{FrameKind, TranscribeStreamEnd};
use speak::ports::presenter::Presenter;

use super::AppFacade;

/// How many chunk POSTs may be in flight at once. Overlaps SSE round trips so the
/// consumer keeps pace with realtime; bounds memory + server load. Pairs with
/// `CHANNEL_CHUNKS` / `POLL_MS` in `coreaudio/macos/stream.rs` (ADR-0018).
pub(crate) const MAX_INFLIGHT: usize = 3;

/// A source of captured chunks the pipeline consumes. Abstracts
/// [`NativeCaptureStream`] so the pipeline ordering can be tested with a fake
/// channel. `next_chunk` MUST be cancel-safe (it is raced in a `select!`).
pub(crate) trait ChunkSource {
    /// The next captured chunk, or `None` once capture has ended.
    async fn next_chunk(&mut self) -> Option<PcmChunk>;
}

impl ChunkSource for NativeCaptureStream {
    async fn next_chunk(&mut self) -> Option<PcmChunk> {
        self.recv().await.map(PcmChunk)
    }
}

/// Drive a streaming-capture session with overlapping in-flight chunk POSTs.
///
/// `build(chunk)` encodes one captured chunk (sync, VAD-gated) and returns the
/// future that POSTs it and yields the lines to print, or `None` for
/// silence/encode-skip. Completed chunks are presented in capture order; a
/// pinned `ctrl_c()` (ADR-0017) cancels an in-flight chunk immediately.
pub(crate) async fn run<S, B, Fut>(
    capture: &mut S,
    presenter: &mut dyn Presenter,
    label: &str,
    mut build: B,
) -> Result<()>
where
    S: ChunkSource,
    B: FnMut(PcmChunk) -> Option<Fut>,
    Fut: Future<Output = Vec<String>>,
{
    let mut inflight = FuturesOrdered::new();
    let mut shutdown = std::pin::pin!(tokio::signal::ctrl_c());
    let mut capture_done = false;
    loop {
        if capture_done && inflight.is_empty() {
            tracing::warn!("capture stream ended");
            return Ok(());
        }
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("{label} stopping");
                return Ok(());
            }
            // Drain the head once it completes — FuturesOrdered preserves capture
            // order, so this never interleaves chunk output.
            Some(lines) = inflight.next(), if !inflight.is_empty() => {
                let lines: Vec<String> = lines;
                for line in &lines {
                    if let Err(e) = presenter.line(line) {
                        tracing::warn!("{label} present failed: {e:#}");
                    }
                }
            }
            // Pull the next chunk only with spare in-flight capacity (backpressure
            // to the channel/ring) and while capture is live.
            chunk = capture.next_chunk(), if !capture_done && inflight.len() < MAX_INFLIGHT => {
                match chunk {
                    Some(raw) => {
                        if let Some(fut) = build(raw) {
                            inflight.push_back(fut);
                        }
                    }
                    None => capture_done = true,
                }
            }
        }
    }
}

/// Newtype wrapper so the `build` closure takes an owned captured chunk without
/// the `cli` modules importing the domain `PcmBuffer` just for the signature.
pub(crate) struct PcmChunk(pub(crate) speak::domain::pcm::PcmBuffer);

/// POST one encoded chunk over a reconnecting SSE stream and collect the text of
/// the `want` frame kind in arrival order. Per-chunk server/transport errors are
/// logged and yield whatever lines arrived — they never abort the session.
pub(crate) async fn collect_chunk(
    facade: &AppFacade,
    sse: &SseRealtimeClient,
    cfg: &Config,
    request: RealtimeRequest,
    want: FrameKind,
) -> Vec<String> {
    let mut stream = sse.stream(request, cfg.retry.policy, cfg.retry.jitter_seed);
    let mut lines = Vec::new();
    let end = facade
        .stream_transcribe_drive(&mut stream, |kind, text| {
            if kind == want {
                lines.push(text.to_owned());
            }
        })
        .await;
    match end {
        Ok(TranscribeStreamEnd::Failed { message }) => {
            tracing::warn!("stream server error: {message}");
        }
        Err(e) => tracing::warn!("stream chunk failed: {e:#}"),
        Ok(_) => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use speak::domain::pcm::PcmBuffer;
    use speak::ports::presenter::{Presenter, Report, Table};
    use tokio::sync::mpsc;

    use super::{ChunkSource, PcmChunk, run};

    /// Test source backed by a real (cancel-safe) tokio channel.
    impl ChunkSource for mpsc::Receiver<PcmBuffer> {
        async fn next_chunk(&mut self) -> Option<PcmChunk> {
            self.recv().await.map(PcmChunk)
        }
    }

    /// Presenter that records every emitted line in order.
    #[derive(Default)]
    struct RecordingPresenter {
        lines: Vec<String>,
    }

    impl Presenter for RecordingPresenter {
        fn report(&mut self, _report: &Report) -> anyhow::Result<()> {
            Ok(())
        }
        fn table(&mut self, _table: &Table) -> anyhow::Result<()> {
            Ok(())
        }
        fn line(&mut self, text: &str) -> anyhow::Result<()> {
            self.lines.push(text.to_owned());
            Ok(())
        }
    }

    fn dummy_chunk() -> PcmBuffer {
        PcmBuffer::new(vec![0.0; 16], 16_000, 1)
    }

    /// Chunks whose round trips finish out of order must still be presented in
    /// capture order (the `FuturesOrdered` invariant, ADR-0018). Earlier chunks are
    /// given LONGER delays so they complete last.
    #[tokio::test]
    async fn presents_chunks_in_capture_order_despite_out_of_order_completion() {
        let (tx, mut rx) = mpsc::channel::<PcmBuffer>(8);
        for _ in 0..5 {
            tx.send(dummy_chunk()).await.unwrap();
        }
        drop(tx); // capture ends after the 5 chunks

        let mut presenter = RecordingPresenter::default();
        let mut index = 0_usize;
        run(&mut rx, &mut presenter, "test", |_chunk| {
            let i = index;
            index += 1;
            // Earlier index → longer delay → completes later than its successors.
            let delay = Duration::from_millis(((10 - i) * 5) as u64);
            Some(async move {
                tokio::time::sleep(delay).await;
                vec![i.to_string()]
            })
        })
        .await
        .unwrap();

        assert_eq!(presenter.lines, vec!["0", "1", "2", "3", "4"]);
    }

    /// `build` returning `None` (silence/encode-skip) drops the chunk silently.
    #[tokio::test]
    async fn skips_chunks_whose_build_returns_none() {
        let (tx, mut rx) = mpsc::channel::<PcmBuffer>(8);
        for _ in 0..4 {
            tx.send(dummy_chunk()).await.unwrap();
        }
        drop(tx);

        let mut presenter = RecordingPresenter::default();
        let mut index = 0_usize;
        run(&mut rx, &mut presenter, "test", |_chunk| {
            let i = index;
            index += 1;
            i.is_multiple_of(2)
                .then_some(async move { vec![i.to_string()] })
        })
        .await
        .unwrap();

        assert_eq!(presenter.lines, vec!["0", "2"]);
    }
}
