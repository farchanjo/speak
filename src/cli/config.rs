//! `config` handler: inspect and initialize the config file.
//!
//! A thin CLI adapter over the `[general]` config resolver (no dedicated use
//! case, per ADR-0003): `path` prints the resolved file location, `show` lists
//! every key with its value + origin, and `init` writes the commented template.

use anyhow::{Context, Result};

use speak::config::{self, Config};
use speak::paths;

use super::args::ConfigAction;

/// Run the `config` subcommand.
pub fn run(action: ConfigAction, cfg: &Config) -> Result<()> {
    let path = paths::config_file();
    match action {
        ConfigAction::Path => println!("{}", path.display()),
        ConfigAction::Show => show(cfg),
        ConfigAction::Init => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            if path.exists() {
                println!("config already exists: {}", path.display());
            } else {
                std::fs::write(&path, config::default_file_toml())?;
                println!("wrote {}", path.display());
            }
        }
    }
    Ok(())
}

/// Print every resolved key with its value and origin.
fn show(cfg: &Config) {
    let width = cfg
        .entries()
        .iter()
        .map(|(k, ..)| k.len())
        .max()
        .unwrap_or(0);
    for (key, value, origin) in cfg.entries() {
        println!("{key:<width$} = {value}  ({origin})");
    }
}
