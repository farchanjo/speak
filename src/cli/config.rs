//! `config` handler: inspect and initialize the config file.
//!
//! A thin CLI adapter over the `[general]` config resolver (no dedicated use
//! case, per ADR-0003): `path` prints the resolved file location, `show` lists
//! every key with its value + origin, and `init` writes the commented template.

use anyhow::{Context, Result};

use speak::config::{self, Config};
use speak::paths;
use speak::ports::presenter::{Presenter, Table};

use super::args::ConfigAction;

/// Run the `config` subcommand, emitting its RESULT through the Presenter.
pub fn run(action: ConfigAction, cfg: &Config, presenter: &mut dyn Presenter) -> Result<()> {
    let path = paths::config_file();
    match action {
        ConfigAction::Path => presenter.line(&path.display().to_string()),
        ConfigAction::Show => show(cfg, presenter),
        ConfigAction::Init => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            if path.exists() {
                presenter.line(&format!("config already exists: {}", path.display()))
            } else {
                std::fs::write(&path, config::default_file_toml())?;
                presenter.line(&format!("wrote {}", path.display()))
            }
        }
    }
}

/// Emit every resolved key with its value and origin as a table.
fn show(cfg: &Config, presenter: &mut dyn Presenter) -> Result<()> {
    let mut table = Table::new(["key", "value", "origin"]);
    for (key, value, origin) in cfg.entries() {
        table = table.row([key.clone(), value.clone(), origin.to_string()]);
    }
    presenter.table(&table)
}
