//! `translate` handler (T051): translate foreign-language audio to text.
//!
//! Reads the file (driving-adapter concern) and drives the [`AppFacade`]'s
//! translate use case (FR-7). The `Translator` Strategy port is text-valued and
//! English-targeted here; subtitle (`srt`/`vtt`) output and arbitrary targets
//! return with the chat-MT Strategy (T039).

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
    // The Translator port targets English; the requested subtitle format returns
    // with the file-translate subtitle path (T039/T041).
    let _ = args.format;
    let target = Language::parse("en")?;
    let text = facade.translate(&bytes, &filename, &target).await?;
    presenter.line(&text)
}
