//! `translate` handler (T051): translate foreign-language audio to text.
//!
//! Reads the file (driving-adapter concern) and drives the [`AppFacade`]'s
//! translate use case (FR-7). The composition root injects the `Translator`
//! **Strategy** the facade holds: an English `--to` stays on Whisper translate,
//! while a non-English target routes through the chat-MT Strategy (T039),
//! degrading to the source transcript when `[http].translate_url` is unset.
//!
//! Subtitle output (`--format srt|vtt`, ADR-0010) is built from the server's
//! transcription SEGMENTS: the `/v1/audio/transcriptions` endpoint emits
//! timestamped SRT/VTT cues, so those formats route through the `Transcriber`
//! port (which already carries the response format) rather than the text
//! translate path. Caveat: subtitle cues are in the SOURCE language (the
//! transcription endpoint times the segments); `--to` applies only to the text
//! formats. Translated subtitles would request SRT/VTT from
//! `/v1/audio/translations` and are a noted enhancement.

use anyhow::{Context, Result};

use speak::adapters::config::Config;
use speak::domain::language::Language;
use speak::ports::presenter::Presenter;
use speak::ports::transcriber::TranscribeRequest;

use super::AppFacade;
use super::args::{TextFormat, TranslateArgs};
use super::file_name;

/// Run the `translate` subcommand, emitting the result through the Presenter.
pub async fn run(
    facade: &AppFacade,
    _cfg: &Config,
    args: TranslateArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("reading {}", args.file.display()))?;
    let filename = file_name(&args.file);
    let text = match args.format {
        TextFormat::Srt | TextFormat::Vtt => {
            subtitles(facade, &bytes, &filename, args.format).await
        }
        _ => translate_text(facade, &bytes, &filename, &args.to).await,
    }?;
    presenter.line(&text)
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
