//! `voices` handler (T051): manage saved cloneable voices (FR-5).
//!
//! Reads the reference audio file (driving-adapter concern) and drives the
//! [`AppFacade`]'s voices use case (add/list/remove).

use anyhow::{Context, Result};

use speak::domain::voice::Voice;
use speak::ports::presenter::{Presenter, Table};

use super::AppFacade;
use super::args::VoicesAction;

/// Run the `voices` subcommand, emitting its RESULT through the Presenter.
pub(crate) async fn run(
    facade: &AppFacade,
    action: VoicesAction,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    match action {
        VoicesAction::List => present_voices(&facade.list_voices().await?, presenter),
        VoicesAction::Add(args) => {
            let bytes = tokio::fs::read(&args.audio)
                .await
                .with_context(|| format!("reading {}", args.audio.display()))?;
            facade
                .add_voice(&args.name, &bytes, args.ref_text.as_deref())
                .await?;
            presenter.line(&format!("added voice {}", args.name))
        }
        VoicesAction::Rm { name } => {
            facade.remove_voice(&name).await?;
            presenter.line(&format!("removed voice {name}"))
        }
    }
}

/// Emit the saved voices as a table, flagging stored reference transcripts.
fn present_voices(voices: &[Voice], presenter: &mut dyn Presenter) -> Result<()> {
    let mut table = Table::new(["voice", "ref_text"]);
    for v in voices {
        let flag = if v.has_ref_text() { "yes" } else { "no" };
        table = table.row([v.name().to_owned(), flag.to_owned()]);
    }
    presenter.table(&table)
}
