//! `transcribe` handler (T051): speech-to-text over a local audio file.
//!
//! Reads the file (driving-adapter concern), builds the [`TranscribeRequest`],
//! and drives the [`AppFacade`]'s transcribe use case (FR-6).

use anyhow::{Context, Result};

use speak::config::Config;
use speak::domain::language::Language;
use speak::ports::presenter::Presenter;
use speak::ports::transcriber::TranscribeRequest;

use super::AppFacade;
use super::args::TranscribeArgs;
use super::file_name;

/// Run the `transcribe` subcommand, emitting the transcript through the Presenter.
pub async fn run(
    facade: &AppFacade,
    cfg: &Config,
    args: TranscribeArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("reading {}", args.file.display()))?;
    let lang = args.language.as_deref().or(cfg.asr.language.as_deref());
    let language = lang.map(Language::parse).transpose()?;
    let filename = file_name(&args.file);
    let req = TranscribeRequest {
        audio: &bytes,
        filename: &filename,
        language: language.as_ref(),
        format: args.format.as_str(),
    };
    let text = facade.transcribe(&req).await?;
    presenter.line(&text)
}
