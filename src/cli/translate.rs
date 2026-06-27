//! `translate` handler (T051): translate foreign-language audio to text.
//!
//! Reads the file (driving-adapter concern) and drives the [`AppFacade`]'s
//! translate use case (FR-7). The composition root injects the `Translator`
//! **Strategy** the facade holds: an English `--to` stays on Whisper translate,
//! while a non-English target routes through the chat-MT Strategy (T039),
//! degrading to the source transcript when `[http].translate_url` is unset.
//! Subtitle (`srt`/`vtt`) output returns with the file-translate subtitle path
//! (T041).

use anyhow::{Context, Result};

use speak::config::Config;
use speak::domain::language::Language;
use speak::ports::presenter::Presenter;

use super::AppFacade;
use super::args::TranslateArgs;
use super::file_name;

/// Run the `translate` subcommand, emitting the translation through the Presenter.
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
    // Subtitle output returns with the file-translate subtitle path (T041).
    let _ = args.format;
    let target = Language::parse(&args.to)?;
    let text = facade.translate(&bytes, &filename, &target).await?;
    presenter.line(&text)
}
