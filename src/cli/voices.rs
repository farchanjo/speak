//! `voices` handler (T051): manage saved cloneable voices (FR-5).
//!
//! Reads the reference audio file (driving-adapter concern) and drives the
//! [`AppFacade`]'s voices use case (add/list/remove).

use anyhow::{Context, Result};

use speak::domain::voice::Voice;

use super::AppFacade;
use super::args::VoicesAction;

/// Run the `voices` subcommand.
pub async fn run(facade: &AppFacade, action: VoicesAction) -> Result<()> {
    match action {
        VoicesAction::List => print_voices(&facade.list_voices().await?),
        VoicesAction::Add(args) => {
            let bytes = tokio::fs::read(&args.audio)
                .await
                .with_context(|| format!("reading {}", args.audio.display()))?;
            facade
                .add_voice(&args.name, &bytes, args.ref_text.as_deref())
                .await?;
            println!("added voice {}", args.name);
        }
        VoicesAction::Rm { name } => {
            facade.remove_voice(&name).await?;
            println!("removed voice {name}");
        }
    }
    Ok(())
}

/// Print the saved voices, flagging those with a stored reference transcript.
fn print_voices(voices: &[Voice]) {
    if voices.is_empty() {
        println!("(no saved voices)");
    }
    for v in voices {
        let tag = if v.has_ref_text() {
            "  (has ref_text)"
        } else {
            ""
        };
        println!("{}{tag}", v.name());
    }
}
