//! `transcribe` handler (T051 / ADR-0014): speech-to-text, file or live stream.
//!
//! File mode reads `FILE`, builds a [`TranscribeRequest`], and drives the
//! [`AppFacade`]'s transcribe use case once (FR-6). `--stream` instead captures
//! live audio from the selected [`CaptureSource`] in chunks and POSTs each to the
//! realtime SSE endpoint with `translate=false`, printing only the `transcript`
//! frames until Ctrl-C — no re-voicing, no playback (FR-1 / FR-2).

use anyhow::{Context, Result, bail};

use speak::adapters::config::Config;
use speak::adapters::sse::{RealtimeRequest, SseRealtimeClient};
use speak::application::{StreamTranscribeOptions, TranscribeStreamEnd};
use speak::domain::capture_source::CaptureSource;
use speak::domain::language::Language;
use speak::ports::presenter::Presenter;
use speak::ports::transcriber::TranscribeRequest;

use super::AppFacade;
use super::args::TranscribeArgs;
use super::file_name;

/// Advertised file name for a captured streaming chunk.
const CHUNK_NAME: &str = "chunk.wav";

/// Run `transcribe` in file mode: emit the transcript through the Presenter.
pub(crate) async fn run(
    facade: &AppFacade,
    cfg: &Config,
    args: TranscribeArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let Some(file) = args.file.as_ref() else {
        bail!("transcribe requires an audio FILE (or use --stream for live capture)");
    };
    let bytes = tokio::fs::read(file)
        .await
        .with_context(|| format!("reading {}", file.display()))?;
    let lang = args.language.as_deref().or(cfg.asr.language.as_deref());
    let language = lang.map(Language::parse).transpose()?;
    let filename = file_name(file);
    let req = TranscribeRequest {
        audio: &bytes,
        filename: &filename,
        language: language.as_ref(),
        format: args.format.as_str(),
    };
    let text = facade.transcribe(&req).await?;
    presenter.line(&text)
}

/// Run `transcribe --stream`: capture live audio and print a live transcript.
pub(crate) async fn run_stream(
    facade: &AppFacade,
    cfg: &Config,
    args: TranscribeArgs,
    sse: &SseRealtimeClient,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let opts = build_options(cfg, &args);
    tracing::info!(
        source = opts.source.direction().as_str(),
        chunk = opts.chunk_secs,
        device = args.device,
        "stream transcribe starting; Ctrl-C to stop"
    );
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("stream transcribe stopping");
                return Ok(());
            }
            res = drive_chunk(facade, cfg, &args, sse, &opts, presenter) => {
                if let Err(e) = res {
                    tracing::warn!("stream transcribe chunk failed: {e:#}");
                }
            }
        }
    }
}

/// Capture one chunk, stream it transcript-only, and surface each transcript.
async fn drive_chunk(
    facade: &AppFacade,
    cfg: &Config,
    args: &TranscribeArgs,
    sse: &SseRealtimeClient,
    opts: &StreamTranscribeOptions,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let Some(wav) = facade.stream_transcribe_capture(opts).await? else {
        return Ok(());
    };
    let request = stream_request(wav, cfg, args);
    let mut stream = sse.stream(request, cfg.retry.policy, cfg.retry.jitter_seed);
    let mut emit_err = None;
    let end = facade
        .stream_transcribe_drive(&mut stream, |text| {
            if let Err(e) = presenter.line(text) {
                emit_err.get_or_insert(e);
            }
        })
        .await?;
    if let TranscribeStreamEnd::Failed { message } = end {
        tracing::warn!("stream transcribe server error: {message}");
    }
    emit_err.map_or(Ok(()), Err)
}

/// Project the captured chunk + config onto a `translate=false` SSE request.
///
/// A voice/format is sent so the server pipeline accepts the chunk exactly as
/// the proven `realtime --no-translate` path does; the re-voiced `audio` frames
/// it returns are ignored client-side (ADR-0014).
fn stream_request(wav: Vec<u8>, cfg: &Config, args: &TranscribeArgs) -> RealtimeRequest {
    let (voice, instruct) = if let Some(tags) = cfg.tts.instruct.clone() {
        (None, Some(tags))
    } else if cfg.tts.voice.is_empty() {
        (None, None)
    } else {
        (Some(cfg.tts.voice.clone()), None)
    };
    RealtimeRequest {
        audio: wav,
        filename: CHUNK_NAME.to_owned(),
        to: None,
        translate: false,
        voice,
        instruct,
        language: args.language.clone().or_else(|| cfg.asr.language.clone()),
        format: cfg.tts.format.clone(),
    }
}

/// Assemble the streaming options from the flags + `[audio.input]` defaults.
fn build_options(cfg: &Config, args: &TranscribeArgs) -> StreamTranscribeOptions {
    let device = (args.device != 0).then_some(args.device);
    let channel = args.input_channel.or(cfg.audio.input.channel);
    let source = CaptureSource::new(args.source.direction(), device, channel);
    let chunk_secs = if args.chunk == 5 {
        cfg.audio.input.chunk_secs
    } else {
        f64::from(args.chunk as u32)
    };
    let vad = cfg.audio.input.vad && !args.no_vad;
    let threshold_db = args
        .vad_floor
        .unwrap_or(cfg.audio.input.silence_threshold_db);
    StreamTranscribeOptions {
        source,
        chunk_secs,
        vad,
        silence_floor: 10f64.powf(threshold_db / 20.0),
    }
}
