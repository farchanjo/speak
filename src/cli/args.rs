//! The clap argument model for the `speak` driving adapter (T050).
//!
//! Pure declaration: every subcommand, its flags, and the `ValueEnum` choices
//! live here so the composition root (`src/main.rs`) and the per-command handlers
//! share one parsed surface. No business logic — the handlers map these values to
//! the application use cases via the [`crate::cli::AppFacade`].

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use speak::config::GlobalFlags;
use speak::daemon;

/// Top-level CLI parser.
#[derive(Parser, Debug)]
#[command(
    name = "speak",
    version,
    about = "Speech client for an OpenAI-compatible TTS/ASR server",
    long_about = "speak synthesizes speech, transcribes and translates audio, and runs a live \
                  microphone translation loop against an OpenAI-compatible speech server. \
                  Audio is decoded with linked ffmpeg/libav and played via the native macOS \
                  CoreAudio mixer.",
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Global options shared by every subcommand.
    #[command(flatten)]
    pub globals: GlobalArgs,
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Global options shared by every subcommand (flags override `SPEAK_*` env).
#[derive(Args, Debug)]
pub struct GlobalArgs {
    /// Server base URL.
    #[arg(long, global = true, env = "SPEAK_HOST", value_name = "URL")]
    pub host: Option<String>,
    /// Bearer API key (sent only when set).
    #[arg(long, global = true, env = "SPEAK_API_KEY", value_name = "KEY")]
    pub api_key: Option<String>,
    /// Language hint (e.g. pt-BR, en).
    #[arg(long, global = true, env = "SPEAK_LANG", value_name = "LANG")]
    pub lang: Option<String>,
    /// TTS voice.
    #[arg(long, global = true, env = "SPEAK_VOICE", value_name = "VOICE")]
    pub voice: Option<String>,
    /// Suppress non-essential status logging.
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,
    /// Emit machine-readable JSON where the command supports it (FR-16).
    #[arg(long, global = true)]
    pub json: bool,
    /// Increase console diagnostics verbosity (repeatable: -v info, -vv debug,
    /// -vvv trace). Diagnostics always go to the rotating `~/.speak/logs` file.
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl GlobalArgs {
    /// Project the global flags into the config resolver's [`GlobalFlags`].
    #[must_use]
    pub fn flags(&self) -> GlobalFlags {
        GlobalFlags {
            host: self.host.clone(),
            api_key: self.api_key.clone(),
            lang: self.lang.clone(),
            voice: self.voice.clone(),
            quiet: self.quiet,
        }
    }
}

/// Every `speak` subcommand.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Synthesize speech and play it locally.
    #[command(alias = "tts")]
    Say(SayArgs),
    /// Transcribe an audio file to text.
    Transcribe(TranscribeArgs),
    /// Translate foreign-language audio to English text.
    Translate(TranslateArgs),
    /// Capture the microphone and translate it live until Ctrl-C.
    Realtime(RealtimeArgs),
    /// Manage saved voices for cloning.
    Voices {
        /// The voices action.
        #[command(subcommand)]
        action: VoicesAction,
    },
    /// Run or control the persistent-connection daemon.
    Daemon(daemon::DaemonArgs),
    /// Probe the OS and local hardware acceleration libav can use.
    Check,
    /// Print the server `/health` JSON.
    Health,
    /// List input/output audio devices and their `AudioDeviceID`s.
    Devices(DevicesArgs),
    /// Manage the config file.
    Config {
        /// The config action.
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Generate a shell completion script.
    Completions {
        /// Target shell.
        #[arg(value_name = "SHELL")]
        shell: Shell,
    },
}

/// `say` arguments.
#[derive(Args, Debug)]
pub struct SayArgs {
    /// Text to speak (reads stdin when omitted).
    #[arg(value_name = "TEXT")]
    pub text: Vec<String>,
    /// Write the encoded audio to a file.
    #[arg(short = 'o', long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Do not play the audio locally.
    #[arg(long)]
    pub no_play: bool,
    /// Speed multiplier.
    #[arg(long, default_value_t = 1.0, value_name = "F")]
    pub speed: f32,
    /// Audio response format (overrides config / SPEAK_FORMAT).
    #[arg(long, value_enum, value_name = "FMT")]
    pub format: Option<AudioFormat>,
    /// Voice design tags, comma-separated (e.g. "Female, British Accent").
    #[arg(long, value_name = "TAGS")]
    pub instruct: Option<String>,
    /// Reference transcript when cloning a saved voice.
    #[arg(long, value_name = "TEXT")]
    pub ref_text: Option<String>,
    /// Target duration hint in seconds.
    #[arg(long, value_name = "SECS")]
    pub duration: Option<f32>,
    /// Repeatable generation param, key=value (e.g. --set num_step=32).
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,
    /// Output device `AudioDeviceID` for playback; repeatable to fan out (FR-11).
    #[arg(long = "output-device", value_name = "ID")]
    pub output_device: Vec<u32>,
    /// Print the valid voice-design tags and exit.
    #[arg(long)]
    pub list_designs: bool,
    /// Use the server's native `/tts` endpoint instead of `/v1/audio/speech`.
    #[arg(long)]
    pub native: bool,
}

/// `transcribe` arguments.
#[derive(Args, Debug)]
pub struct TranscribeArgs {
    /// Audio file to transcribe.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// Source language hint.
    #[arg(long, value_name = "LANG")]
    pub language: Option<String>,
    /// Transcript output format.
    #[arg(long, value_enum, default_value_t = TextFormat::Text)]
    pub format: TextFormat,
}

/// `translate` arguments.
#[derive(Args, Debug)]
pub struct TranslateArgs {
    /// Audio file to translate to English.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = TextFormat::Text)]
    pub format: TextFormat,
}

/// `realtime` arguments.
#[derive(Args, Debug)]
pub struct RealtimeArgs {
    /// Source language hint (auto-detect when omitted).
    #[arg(long, value_name = "LANG")]
    pub from: Option<String>,
    /// Target language (`en` uses Whisper translate directly).
    #[arg(long, default_value = "en", value_name = "LANG")]
    pub to: String,
    /// Re-voice the source transcript without translating it.
    #[arg(long)]
    pub repeat: bool,
    /// Synthesize and play the result through the speaker.
    #[arg(long)]
    pub speak: bool,
    /// Voice design tags for the spoken output (e.g. "Female, British Accent").
    #[arg(long, value_name = "TAGS")]
    pub instruct: Option<String>,
    /// Chunk length in seconds.
    #[arg(long, default_value_t = 5, value_name = "SECS")]
    pub chunk: u64,
    /// Input device index (0 = system default).
    #[arg(long, default_value_t = 0, value_name = "IDX")]
    pub device: u32,
}

/// `devices` arguments.
#[derive(Args, Debug)]
pub struct DevicesArgs {
    /// Emit the device list as JSON.
    #[arg(long)]
    pub json: bool,
}

/// `config` actions.
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Write a default config file if absent.
    Init,
    /// Print the config file path.
    Path,
    /// Print the effective (resolved) configuration.
    Show,
}

/// `voices` actions.
#[derive(Subcommand, Debug)]
pub enum VoicesAction {
    /// Save a voice from a reference audio file.
    Add(VoiceAddArgs),
    /// List saved voices.
    List,
    /// Delete a saved voice.
    Rm {
        /// Voice name.
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// `voices add` arguments.
#[derive(Args, Debug)]
pub struct VoiceAddArgs {
    /// Voice name to register.
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Reference audio file.
    #[arg(long, value_name = "FILE")]
    pub audio: PathBuf,
    /// Reference transcript for the audio.
    #[arg(long, value_name = "TEXT")]
    pub ref_text: Option<String>,
}

/// OpenAI audio response formats.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum AudioFormat {
    /// MPEG-1 Audio Layer III.
    Mp3,
    /// Opus in an Ogg container.
    Opus,
    /// Advanced Audio Coding.
    Aac,
    /// Free Lossless Audio Codec.
    Flac,
    /// RIFF/WAVE PCM.
    Wav,
    /// Raw little-endian PCM.
    Pcm,
}

impl AudioFormat {
    /// The canonical wire token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Aac => "aac",
            Self::Flac => "flac",
            Self::Wav => "wav",
            Self::Pcm => "pcm",
        }
    }
}

/// Transcript/translation text formats.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum TextFormat {
    /// Plain text.
    Text,
    /// JSON envelope.
    Json,
    /// SubRip subtitles.
    Srt,
    /// WebVTT subtitles.
    Vtt,
    /// Verbose JSON with timestamps.
    #[value(name = "verbose_json")]
    VerboseJson,
}

impl TextFormat {
    /// The canonical wire token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::VerboseJson => "verbose_json",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // Guards against clap derive misconfiguration (duplicate args, etc.).
        Cli::command().debug_assert();
    }

    #[test]
    fn audio_format_wire_strings() {
        assert_eq!(AudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(AudioFormat::Opus.as_str(), "opus");
        assert_eq!(AudioFormat::Aac.as_str(), "aac");
        assert_eq!(AudioFormat::Flac.as_str(), "flac");
        assert_eq!(AudioFormat::Wav.as_str(), "wav");
        assert_eq!(AudioFormat::Pcm.as_str(), "pcm");
    }

    #[test]
    fn text_format_wire_strings() {
        assert_eq!(TextFormat::Text.as_str(), "text");
        assert_eq!(TextFormat::Json.as_str(), "json");
        assert_eq!(TextFormat::Srt.as_str(), "srt");
        assert_eq!(TextFormat::Vtt.as_str(), "vtt");
        assert_eq!(TextFormat::VerboseJson.as_str(), "verbose_json");
    }
}
