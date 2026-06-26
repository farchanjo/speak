//! Configuration with strict precedence: CLI flags > environment
//! (`SPEAK_*`) > TOML (`~/.config/speak/config.toml`) > built-in defaults.
//!
//! clap resolves flags-and-env for the global options (so `--help` documents
//! the env vars); this module layers the TOML file and defaults underneath.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Default server base URL.
pub const DEFAULT_HOST: &str = "http://solaris:8800";
/// Default synthesis/recognition language.
pub const DEFAULT_LANG: &str = "pt-BR";
/// Default TTS voice.
pub const DEFAULT_VOICE: &str = "alloy";
/// Default OpenAI `response_format`.
pub const DEFAULT_FORMAT: &str = "mp3";
/// Default TTS model id.
pub const DEFAULT_TTS_MODEL: &str = "tts-1";
/// Default ASR model id.
pub const DEFAULT_ASR_MODEL: &str = "whisper-1";

/// On-disk configuration; every field is optional and overrides defaults.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FileConfig {
    /// Server base URL.
    pub host: Option<String>,
    /// Bearer API key.
    pub api_key: Option<String>,
    /// Default language.
    pub lang: Option<String>,
    /// Default voice.
    pub voice: Option<String>,
    /// Default `response_format`.
    pub format: Option<String>,
    /// TTS model id.
    pub tts_model: Option<String>,
    /// ASR model id.
    pub asr_model: Option<String>,
    /// Optional chat-completions endpoint for arbitrary-target translation.
    pub translate_url: Option<String>,
    /// Optional chat model id for arbitrary-target translation.
    pub translate_model: Option<String>,
}

/// Fully-resolved effective configuration.
#[derive(Debug, Clone, Serialize)]
pub struct Config {
    /// Server base URL.
    pub host: String,
    /// Bearer API key, if any.
    pub api_key: Option<String>,
    /// Effective language.
    pub lang: String,
    /// Effective voice.
    pub voice: String,
    /// Effective `response_format`.
    pub format: String,
    /// TTS model id.
    pub tts_model: String,
    /// ASR model id.
    pub asr_model: String,
    /// Optional chat-completions translation endpoint.
    pub translate_url: Option<String>,
    /// Optional chat translation model id.
    pub translate_model: Option<String>,
}

/// Global option values as parsed by clap (flag-or-env, else `None`).
///
/// `--format` is intentionally not global: its meaning differs per subcommand
/// (audio response format for `say`, text format for `transcribe`), so the
/// audio default is resolved from `SPEAK_FORMAT`/TOML here and overridden by
/// the `say --format` flag.
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    /// `--host` / `SPEAK_HOST`.
    pub host: Option<String>,
    /// `--api-key` / `SPEAK_API_KEY`.
    pub api_key: Option<String>,
    /// `--lang` / `SPEAK_LANG`.
    pub lang: Option<String>,
    /// `--voice` / `SPEAK_VOICE`.
    pub voice: Option<String>,
}

/// Resolve the config-file path, honouring `XDG_CONFIG_HOME`.
#[must_use]
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("speak").join("config.toml")
}

/// Load the TOML config file, returning an empty config when it is absent.
pub fn load_file() -> Result<FileConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}

fn pick(flag: Option<String>, file: Option<String>, default: &str) -> String {
    flag.or(file).unwrap_or_else(|| default.to_owned())
}

impl Config {
    /// Layer CLI overrides over the file config over the built-in defaults.
    #[must_use]
    pub fn resolve(cli: Overrides, file: FileConfig) -> Self {
        Self {
            host: pick(cli.host, file.host, DEFAULT_HOST),
            api_key: cli.api_key.or(file.api_key),
            lang: pick(cli.lang, file.lang, DEFAULT_LANG),
            voice: pick(cli.voice, file.voice, DEFAULT_VOICE),
            format: pick(
                std::env::var("SPEAK_FORMAT").ok(),
                file.format,
                DEFAULT_FORMAT,
            ),
            tts_model: file
                .tts_model
                .unwrap_or_else(|| DEFAULT_TTS_MODEL.to_owned()),
            asr_model: file
                .asr_model
                .unwrap_or_else(|| DEFAULT_ASR_MODEL.to_owned()),
            translate_url: file.translate_url,
            translate_model: file.translate_model,
        }
    }
}

/// Render the default config file body (commented template).
#[must_use]
pub fn default_file_toml() -> String {
    format!(
        "# speak configuration ({path})\n\
         host = \"{DEFAULT_HOST}\"\n\
         # api_key = \"sk-...\"\n\
         lang = \"{DEFAULT_LANG}\"\n\
         voice = \"{DEFAULT_VOICE}\"\n\
         format = \"{DEFAULT_FORMAT}\"\n\
         tts_model = \"{DEFAULT_TTS_MODEL}\"\n\
         asr_model = \"{DEFAULT_ASR_MODEL}\"\n\
         # translate_url = \"http://solaris:8800/v1/chat/completions\"\n\
         # translate_model = \"gpt-4o-mini\"\n",
        path = config_path().display(),
    )
}
