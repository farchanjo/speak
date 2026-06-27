//! `completions` handler: emit a shell completion script for `speak`.

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{Shell, generate};

use super::args::Cli;

/// Generate and print the completion script for `shell`.
pub fn emit(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "speak", &mut std::io::stdout());
    Ok(())
}
