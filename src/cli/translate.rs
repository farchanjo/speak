//! `translate` handler (T051 / ADR-0014+0017): file or live-stream translation.
//!
//! File mode reads `FILE` and drives the [`AppFacade`]'s translate use case
//! (FR-7): an English `--to` stays on Whisper translate, a non-English target
//! routes through the chat-MT **Strategy** (T039), degrading to the source
//! transcript when `[http].translate_url` is unset. `--format srt|vtt` builds
//! subtitle cues from the transcription SEGMENTS (in the SOURCE language, ADR-0010).
//!
//! `--stream` instead captures live audio **continuously** (ADR-0017) and POSTs
//! each chunk to the realtime SSE endpoint with `translate=true`, printing the
//! `translation` frames until Ctrl-C — the streaming sibling of
//! `transcribe --stream` (which prints the `transcript` frames of the same
//! endpoint with `translate=false`).

use anyhow::{Context, Result, bail};

use speak::adapters::config::Config;
use speak::adapters::coreaudio::CoreAudio;
use speak::adapters::sse::{RealtimeRequest, SseRealtimeClient};
use speak::application::FrameKind;
use speak::domain::language::Language;
use speak::ports::presenter::Presenter;
use speak::ports::transcriber::TranscribeRequest;

use super::AppFacade;
use super::args::{TextFormat, TranslateArgs};
use super::file_name;
use super::stream_pipeline;

/// Advertised file name for a captured streaming chunk.
const CHUNK_NAME: &str = "chunk.wav";

/// Run `translate` in file mode, emitting the result through the Presenter.
pub(crate) async fn run(
    facade: &AppFacade,
    _cfg: &Config,
    args: TranslateArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let Some(file) = args.file.as_ref() else {
        bail!("translate requires an audio FILE (or use --stream for live capture)");
    };
    let bytes = tokio::fs::read(file)
        .await
        .with_context(|| format!("reading {}", file.display()))?;
    let filename = file_name(file);
    let text = match args.format {
        TextFormat::Srt | TextFormat::Vtt => {
            subtitles(facade, &bytes, &filename, args.format).await
        }
        _ => translate_text(facade, &bytes, &filename, &args.to).await,
    }?;
    presenter.line(&text)
}

/// Run `translate --stream`: capture live audio and print a live translation.
pub(crate) async fn run_stream(
    facade: &AppFacade,
    cfg: &Config,
    args: TranslateArgs,
    sse: &SseRealtimeClient,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let opts = super::stream_options(
        args.source,
        args.device,
        args.input_channel,
        args.chunk,
        args.no_vad,
        args.vad_floor,
        cfg,
    );
    let mut capture = CoreAudio::new().capture_stream(
        &opts.source,
        opts.chunk_secs,
        cfg.audio.capture.buffer_secs,
    )?;
    tracing::info!(
        source = opts.source.direction().as_str(),
        chunk = opts.chunk_secs,
        to = %args.to,
        "stream translate starting; Ctrl-C to stop"
    );
    // Pipeline up to MAX_INFLIGHT chunk POSTs, presenting translations in capture
    // order (ADR-0018); a slow round trip no longer stalls the consumer.
    stream_pipeline::run(&mut capture, presenter, "stream translate", |chunk| {
        let wav = match facade.stream_transcribe_encode(chunk.0, &opts) {
            Ok(Some(wav)) => wav,
            Ok(None) => return None, // silence — VAD-gated
            Err(e) => {
                tracing::warn!("stream translate encode failed: {e:#}");
                return None;
            }
        };
        let request = stream_request(wav, cfg, &args);
        Some(stream_pipeline::collect_chunk(
            facade,
            sse,
            cfg,
            request,
            FrameKind::Translation,
        ))
    })
    .await
}

/// Project the captured chunk + config onto a `translate=true` SSE request.
fn stream_request(wav: Vec<u8>, cfg: &Config, args: &TranslateArgs) -> RealtimeRequest {
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
        to: Some(args.to.clone()),
        translate: true,
        voice,
        instruct,
        language: cfg.asr.language.clone(),
        format: cfg.tts.format.clone(),
    }
}

/// Translate to plain text in the `--to` target (Whisper-English or chat-MT, T039).
async fn translate_text(
    facade: &AppFacade,
    bytes: &[u8],
    filename: &str,
    to: &str,
) -> Result<String> {
    let target = Language::parse(to)?;
    facade.translate(bytes, filename, &target).await
}

/// Emit timestamped subtitle cues from the server's transcription segments
/// (`/v1/audio/transcriptions` with `response_format=srt|vtt`, ADR-0010).
async fn subtitles(
    facade: &AppFacade,
    bytes: &[u8],
    filename: &str,
    format: TextFormat,
) -> Result<String> {
    let req = TranscribeRequest {
        audio: bytes,
        filename,
        language: None,
        format: format.as_str(),
    };
    facade.transcribe(&req).await
}
