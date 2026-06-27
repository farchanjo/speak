//! The clap argument model for the `speak` driving adapter (T050).
//!
//! Pure declaration: every subcommand, its flags, and the `ValueEnum` choices
//! live here so the composition root (`src/main.rs`) and the per-command handlers
//! share one parsed surface. No business logic — the handlers map these values to
//! the application use cases via the [`crate::cli::AppFacade`].

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use speak::adapters::config::GlobalFlags;
use speak::adapters::daemon;

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
pub(crate) struct Cli {
    /// Global options shared by every subcommand.
    #[command(flatten)]
    pub globals: GlobalArgs,
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Global options shared by every subcommand (flags override `SPEAK_*` env).
#[derive(Args, Debug)]
pub(crate) struct GlobalArgs {
    /// Server base URL.
    #[arg(
        short = 'H',
        long,
        global = true,
        env = "SPEAK_HOST",
        value_name = "URL"
    )]
    pub host: Option<String>,
    /// Bearer API key (sent only when set).
    #[arg(
        short = 'K',
        long,
        global = true,
        env = "SPEAK_API_KEY",
        value_name = "KEY"
    )]
    pub api_key: Option<String>,
    /// Language hint (e.g. pt-BR, en).
    #[arg(
        short = 'L',
        long,
        global = true,
        env = "SPEAK_LANG",
        value_name = "LANG"
    )]
    pub lang: Option<String>,
    /// TTS voice (`-v`/`-V` are taken by verbose/version).
    #[arg(
        short = 'C',
        long,
        global = true,
        env = "SPEAK_VOICE",
        value_name = "VOICE"
    )]
    pub voice: Option<String>,
    /// Suppress non-essential status logging.
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,
    /// Emit machine-readable JSON where the command supports it (FR-16).
    #[arg(short = 'J', long, global = true)]
    pub json: bool,
    /// Increase console diagnostics verbosity (repeatable: -v info, -vv debug,
    /// -vvv trace). Diagnostics always go to the rotating `~/.speak/logs` file.
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl GlobalArgs {
    /// Project the global flags into the config resolver's [`GlobalFlags`].
    #[must_use]
    pub(crate) fn flags(&self) -> GlobalFlags {
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
pub(crate) enum Command {
    /// Synthesize speech and play it locally.
    #[command(alias = "tts")]
    Say(SayArgs),
    /// Transcribe an audio file to text.
    Transcribe(TranscribeArgs),
    /// Translate foreign-language audio to English text.
    Translate(TranslateArgs),
    /// Capture the microphone and translate it live until Ctrl-C.
    Realtime(RealtimeArgs),
    /// Record the microphone to a WAV/FLAC file.
    Record(RecordArgs),
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
pub(crate) struct SayArgs {
    /// Text to speak (reads stdin when omitted).
    #[arg(value_name = "TEXT")]
    pub text: Vec<String>,
    /// Write the encoded audio to a file.
    #[arg(short = 'o', long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Do not play the audio locally.
    #[arg(short = 'n', long)]
    pub no_play: bool,
    /// Speed multiplier.
    #[arg(short = 's', long, default_value_t = 1.0, value_name = "F")]
    pub speed: f32,
    /// Audio response format (overrides config / `SPEAK_FORMAT`).
    #[arg(short = 'f', long, value_enum, value_name = "FMT")]
    pub format: Option<AudioFormat>,
    /// Voice design tags, comma-separated (e.g. "Female, British Accent").
    #[arg(short = 'i', long, value_name = "TAGS")]
    pub instruct: Option<String>,
    /// Reference transcript when cloning a saved voice.
    #[arg(short = 'r', long, value_name = "TEXT")]
    pub ref_text: Option<String>,
    /// Target duration hint in seconds.
    #[arg(short = 'd', long, value_name = "SECS")]
    pub duration: Option<f32>,
    /// Repeatable generation param, key=value (e.g. --set `num_step=32`).
    #[arg(short = 'S', long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,
    /// Output device `AudioDeviceID` for playback; repeatable to fan out (FR-11).
    #[arg(short = 'D', long = "output-device", value_name = "ID")]
    pub output_device: Vec<u32>,
    /// Print the valid voice-design tags and exit.
    #[arg(short = 'g', long)]
    pub list_designs: bool,
    /// Use the server's native `/tts` endpoint instead of `/v1/audio/speech`.
    #[arg(short = 'N', long)]
    pub native: bool,
}

/// `transcribe` arguments.
#[derive(Args, Debug)]
pub(crate) struct TranscribeArgs {
    /// Audio file to transcribe.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// Source language hint.
    #[arg(short = 'l', long, value_name = "LANG")]
    pub language: Option<String>,
    /// Transcript output format.
    #[arg(short = 'f', long, value_enum, default_value_t = TextFormat::Text)]
    pub format: TextFormat,
}

/// `translate` arguments.
#[derive(Args, Debug)]
pub(crate) struct TranslateArgs {
    /// Audio file to translate.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// Target language (`en` uses Whisper translate; others use chat-MT, T039).
    #[arg(short = 't', long, default_value = "en", value_name = "LANG")]
    pub to: String,
    /// Output format.
    #[arg(short = 'f', long, value_enum, default_value_t = TextFormat::Text)]
    pub format: TextFormat,
}

/// `realtime` arguments.
///
/// The pipeline mode is an exclusive group of `--translate` (default),
/// `--no-translate`, and `--echo` (FR-8); the spoken output voice is selected by
/// `--instruct` (design) or the global `--voice` (clone / standard).
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("realtime_mode")
        .multiple(false)
        .args(["translate", "no_translate", "echo"])
))]
#[expect(
    clippy::struct_excessive_bools,
    reason = "clap flag struct: the mutually-exclusive mode group plus the VAD toggle are independent CLI switches, not a state enum"
)]
pub(crate) struct RealtimeArgs {
    /// Source language hint (auto-detect when omitted).
    #[arg(short = 'f', long, value_name = "LANG")]
    pub from: Option<String>,
    /// Target language for `--translate` (`en` uses Whisper translate directly).
    #[arg(short = 't', long, default_value = "en", value_name = "LANG")]
    pub to: String,
    /// Translate the source speech, then re-voice the translation (default).
    #[arg(short = 'T', long)]
    pub translate: bool,
    /// Re-voice the source transcript without translating it.
    #[arg(short = 'n', long = "no-translate")]
    pub no_translate: bool,
    /// Play the raw capture back, then re-voice it.
    #[arg(short = 'e', long)]
    pub echo: bool,
    /// Voice design tags for the spoken output (e.g. "Female, British Accent").
    #[arg(short = 'i', long, value_name = "TAGS")]
    pub instruct: Option<String>,
    /// Output device `AudioDeviceID` for playback; repeatable to fan out (FR-11).
    #[arg(short = 'D', long = "output-device", value_name = "ID")]
    pub output_device: Vec<u32>,
    /// Chunk length in seconds.
    #[arg(short = 'c', long, default_value_t = 5, value_name = "SECS")]
    pub chunk: u64,
    /// Capture device `AudioDeviceID` (0 = system default input).
    #[arg(short = 'd', long, default_value_t = 0, value_name = "ID")]
    pub device: u32,
    /// Disable the silence/VAD gate (send every captured chunk).
    #[arg(short = 'x', long = "no-vad")]
    pub no_vad: bool,
    /// Silence-gate threshold in dBFS for this run (overrides config). dBFS is
    /// negative (e.g. -50), so hyphen-prefixed values are accepted.
    #[arg(
        short = 'F',
        long = "vad-floor",
        value_name = "DBFS",
        allow_negative_numbers = true
    )]
    pub vad_floor: Option<f64>,
    /// Capture only this 0-based input channel before the mono downmix — for a
    /// mic on one input of a multi-channel interface (e.g. SSL 12 input 1 = 0).
    #[arg(short = 'I', long = "input-channel", value_name = "N")]
    pub input_channel: Option<u16>,
}

impl RealtimeArgs {
    /// Resolve the selected pipeline mode (default `Translate`).
    #[must_use]
    pub(crate) fn mode(&self) -> speak::domain::realtime::RealtimeMode {
        use speak::domain::realtime::RealtimeMode;
        if self.no_translate {
            RealtimeMode::NoTranslate
        } else if self.echo {
            RealtimeMode::Echo
        } else {
            RealtimeMode::Translate
        }
    }
}

/// `record` arguments (FR-9).
#[derive(Args, Debug)]
pub(crate) struct RecordArgs {
    /// Output file (`.wav`/`.flac`).
    #[arg(short = 'o', long, value_name = "FILE")]
    pub output: PathBuf,
    /// Capture duration in seconds.
    #[arg(short = 'd', long, value_name = "SECS")]
    pub duration: f64,
    /// Capture device `AudioDeviceID` (omit for the system default input).
    #[arg(short = 'D', long, value_name = "ID")]
    pub device: Option<u32>,
    /// Output container.
    #[arg(short = 'f', long, value_enum, default_value_t = RecordFormatArg::Wav)]
    pub format: RecordFormatArg,
    /// Resample to this sample rate (Hz); omit to keep the captured rate.
    #[arg(short = 'r', long, value_name = "HZ")]
    pub sample_rate: Option<u32>,
    /// Resample to this channel count; omit to keep the captured channels.
    #[arg(short = 'c', long, value_name = "N")]
    pub channels: Option<u16>,
    /// Capture only this 0-based input channel before resampling — for a mic on
    /// one input of a multi-channel interface (e.g. SSL 12 input 1 = 0).
    #[arg(short = 'I', long = "input-channel", value_name = "N")]
    pub input_channel: Option<u16>,
}

/// `record` output containers (maps to the codec port's `RecordFormat`).
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum RecordFormatArg {
    /// RIFF/WAVE PCM.
    Wav,
    /// Free Lossless Audio Codec.
    Flac,
}

impl RecordFormatArg {
    /// Project the CLI choice onto the codec port's `RecordFormat`.
    #[must_use]
    pub(crate) fn to_record_format(self) -> speak::ports::codec::RecordFormat {
        use speak::ports::codec::RecordFormat;
        match self {
            Self::Wav => RecordFormat::Wav,
            Self::Flac => RecordFormat::Flac,
        }
    }
}

/// `devices` arguments.
#[derive(Args, Debug)]
pub(crate) struct DevicesArgs {
    /// Emit the device list as JSON.
    #[arg(long)]
    pub json: bool,
}

/// `config` actions.
#[derive(Subcommand, Debug, Clone, Copy)]
pub(crate) enum ConfigAction {
    /// Write a default config file if absent.
    Init,
    /// Print the config file path.
    Path,
    /// Print the effective (resolved) configuration.
    Show,
}

/// `voices` actions.
#[derive(Subcommand, Debug)]
pub(crate) enum VoicesAction {
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
pub(crate) struct VoiceAddArgs {
    /// Voice name to register.
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Reference audio file.
    #[arg(short = 'a', long, value_name = "FILE")]
    pub audio: PathBuf,
    /// Reference transcript for the audio.
    #[arg(short = 'r', long, value_name = "TEXT")]
    pub ref_text: Option<String>,
}

/// `OpenAI` audio response formats.
#[derive(Copy, Clone, Debug, ValueEnum)]
pub(crate) enum AudioFormat {
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
    pub(crate) fn as_str(self) -> &'static str {
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
pub(crate) enum TextFormat {
    /// Plain text.
    Text,
    /// JSON envelope.
    Json,
    /// `SubRip` subtitles.
    Srt,
    /// `WebVTT` subtitles.
    Vtt,
    /// Verbose JSON with timestamps.
    #[value(name = "verbose_json")]
    VerboseJson,
}

impl TextFormat {
    /// The canonical wire token.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
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

    #[test]
    fn realtime_accepts_negative_vad_floor() {
        use clap::Parser;
        // dBFS is negative; the flag must accept a hyphen-prefixed value.
        let cli = Cli::try_parse_from(["speak", "realtime", "--vad-floor", "-50", "--no-vad"])
            .expect("negative --vad-floor must parse");
        match cli.command {
            Command::Realtime(a) => {
                assert_eq!(a.vad_floor, Some(-50.0));
                assert!(a.no_vad);
            }
            other => panic!("expected realtime, got {other:?}"),
        }
    }

    #[test]
    fn realtime_mode_defaults_to_translate_and_honours_flags() {
        use speak::domain::realtime::RealtimeMode;
        let make = |translate, no_translate, echo| RealtimeArgs {
            from: None,
            to: "en".to_owned(),
            translate,
            no_translate,
            echo,
            instruct: None,
            output_device: Vec::new(),
            chunk: 5,
            device: 0,
            no_vad: false,
            vad_floor: None,
            input_channel: None,
        };
        assert_eq!(make(false, false, false).mode(), RealtimeMode::Translate);
        assert_eq!(make(true, false, false).mode(), RealtimeMode::Translate);
        assert_eq!(make(false, true, false).mode(), RealtimeMode::NoTranslate);
        assert_eq!(make(false, false, true).mode(), RealtimeMode::Echo);
    }
}
