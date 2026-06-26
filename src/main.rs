//! `speak` — a network client for an OpenAI-compatible speech server.
//!
//! Media path: server audio is decoded and resampled with linked `libav*`
//! (ffmpeg-the-third) and played through the native macOS CoreAudio mixer
//! (AVAudioEngine); the microphone is captured natively too. Nothing is
//! shelled out.

mod client;
mod codec;
mod config;

#[cfg(target_os = "macos")]
#[path = "audio_macos.rs"]
mod audio;
#[cfg(not(target_os = "macos"))]
#[path = "audio_stub.rs"]
mod audio;

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{generate, Shell};

use crate::client::{SpeakRequest, SpeechClient};
use crate::config::{Config, Overrides};

/// RMS threshold below which a captured chunk is treated as silence.
const SILENCE_RMS: f64 = 0.012;

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
struct Cli {
    #[command(flatten)]
    globals: GlobalArgs,
    #[command(subcommand)]
    command: Command,
}

/// Global options shared by every subcommand (flags override `SPEAK_*` env).
#[derive(Args, Debug)]
struct GlobalArgs {
    /// Server base URL.
    #[arg(long, global = true, env = "SPEAK_HOST", value_name = "URL")]
    host: Option<String>,
    /// Bearer API key (sent only when set).
    #[arg(long, global = true, env = "SPEAK_API_KEY", value_name = "KEY")]
    api_key: Option<String>,
    /// Language hint (e.g. pt-BR, en).
    #[arg(long, global = true, env = "SPEAK_LANG", value_name = "LANG")]
    lang: Option<String>,
    /// TTS voice.
    #[arg(long, global = true, env = "SPEAK_VOICE", value_name = "VOICE")]
    voice: Option<String>,
    /// Suppress non-essential status logging.
    #[arg(short = 'q', long, global = true)]
    quiet: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Synthesize speech and play it locally.
    #[command(alias = "tts")]
    Say(SayArgs),
    /// Transcribe an audio file to text.
    Transcribe(TranscribeArgs),
    /// Translate foreign-language audio to English text.
    Translate(TranslateArgs),
    /// Capture the microphone and translate it live until Ctrl-C.
    Realtime(RealtimeArgs),
    /// Print the server `/health` JSON.
    Health,
    /// Manage the config file.
    Config {
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

#[derive(Args, Debug)]
struct SayArgs {
    /// Text to speak (reads stdin when omitted).
    #[arg(value_name = "TEXT")]
    text: Vec<String>,
    /// Write the encoded audio to a file.
    #[arg(short = 'o', long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Do not play the audio locally.
    #[arg(long)]
    no_play: bool,
    /// Speed multiplier.
    #[arg(long, default_value_t = 1.0, value_name = "F")]
    speed: f32,
    /// Audio response format (overrides config / SPEAK_FORMAT).
    #[arg(long, value_enum, value_name = "FMT")]
    format: Option<AudioFormat>,
    /// Use the server's native `/tts` endpoint instead of `/v1/audio/speech`.
    #[arg(long)]
    native: bool,
}

#[derive(Args, Debug)]
struct TranscribeArgs {
    /// Audio file to transcribe.
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Source language hint.
    #[arg(long, value_name = "LANG")]
    language: Option<String>,
    /// Transcript output format.
    #[arg(long, value_enum, default_value_t = TextFormat::Text)]
    format: TextFormat,
}

#[derive(Args, Debug)]
struct TranslateArgs {
    /// Audio file to translate to English.
    #[arg(value_name = "FILE")]
    file: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = TextFormat::Text)]
    format: TextFormat,
}

#[derive(Args, Debug)]
struct RealtimeArgs {
    /// Source language hint (auto-detect when omitted).
    #[arg(long, value_name = "LANG")]
    from: Option<String>,
    /// Target language (`en` uses Whisper translate directly).
    #[arg(long, default_value = "en", value_name = "LANG")]
    to: String,
    /// Also synthesize and play the translation.
    #[arg(long)]
    speak: bool,
    /// Chunk length in seconds.
    #[arg(long, default_value_t = 5, value_name = "SECS")]
    chunk: u64,
    /// Input device index (0 = system default).
    #[arg(long, default_value_t = 0, value_name = "IDX")]
    device: u32,
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Write a default config file if absent.
    Init,
    /// Print the config file path.
    Path,
    /// Print the effective (resolved) configuration.
    Show,
}

/// OpenAI audio response formats.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum AudioFormat {
    Mp3,
    Opus,
    Aac,
    Flac,
    Wav,
    Pcm,
}

impl AudioFormat {
    fn as_str(self) -> &'static str {
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
enum TextFormat {
    Text,
    Json,
    Srt,
    Vtt,
    #[value(name = "verbose_json")]
    VerboseJson,
}

impl TextFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::VerboseJson => "verbose_json",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> Result<()> {
    if let Command::Completions { shell } = cli.command {
        return emit_completions(shell);
    }
    let cfg = Config::resolve(overrides(&cli.globals), config::load_file()?);
    match cli.command {
        Command::Health => cmd_health(&cfg).await,
        Command::Config { action } => cmd_config(action, &cfg),
        Command::Say(args) => cmd_say(&cfg, &cli.globals, args).await,
        Command::Transcribe(args) => cmd_transcribe(&cfg, args).await,
        Command::Translate(args) => cmd_translate(&cfg, args).await,
        Command::Realtime(args) => cmd_realtime(&cfg, &cli.globals, args).await,
        Command::Completions { .. } => Ok(()),
    }
}

fn overrides(g: &GlobalArgs) -> Overrides {
    Overrides {
        host: g.host.clone(),
        api_key: g.api_key.clone(),
        lang: g.lang.clone(),
        voice: g.voice.clone(),
    }
}

fn emit_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "speak", &mut std::io::stdout());
    Ok(())
}

async fn cmd_health(cfg: &Config) -> Result<()> {
    let client = SpeechClient::new(cfg)?;
    let value = client.health().await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_config(action: ConfigAction, cfg: &Config) -> Result<()> {
    let path = config::config_path();
    match action {
        ConfigAction::Path => println!("{}", path.display()),
        ConfigAction::Show => print!("{}", toml::to_string_pretty(cfg)?),
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

async fn cmd_say(cfg: &Config, globals: &GlobalArgs, args: SayArgs) -> Result<()> {
    let client = SpeechClient::new(cfg)?;
    let text = resolve_text(args.text)?;
    let response_format = args.format.map_or_else(|| cfg.format.clone(), |f| f.as_str().to_owned());
    let reply = if args.native {
        client.speak_native(&text, &cfg.lang, args.speed).await?
    } else {
        let req = SpeakRequest {
            input: &text,
            model: &cfg.tts_model,
            voice: &cfg.voice,
            response_format: &response_format,
            speed: args.speed,
            language: &cfg.lang,
        };
        client.speak(&req).await?
    };
    if let Some(path) = &args.out {
        tokio::fs::write(path, &reply.bytes).await?;
        if !globals.quiet {
            eprintln!("saved {} bytes to {}", reply.bytes.len(), path.display());
        }
    }
    if !args.no_play {
        play_bytes(reply.bytes, reply.content_type, globals.quiet).await?;
    }
    Ok(())
}

async fn play_bytes(bytes: Vec<u8>, content_type: String, quiet: bool) -> Result<()> {
    let (samples, frames, secs) = tokio::task::spawn_blocking(move || -> Result<_> {
        let pcm = codec::decode(bytes)?;
        let stats = (pcm.samples.len(), pcm.frames(), pcm.duration_secs());
        audio::play(&pcm)?;
        Ok(stats)
    })
    .await??;
    if !quiet {
        eprintln!(
            "decoded {content_type}: {samples} samples ({frames} frames @ {}Hz, {secs:.2}s); \
             played via native CoreAudio mixer",
            codec::PLAY_RATE
        );
    }
    Ok(())
}

async fn cmd_transcribe(cfg: &Config, args: TranscribeArgs) -> Result<()> {
    let client = SpeechClient::new(cfg)?;
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("reading {}", args.file.display()))?;
    let text = client
        .transcribe(bytes, &file_name(&args.file), &cfg.asr_model, args.language.as_deref(), args.format.as_str())
        .await?;
    println!("{text}");
    Ok(())
}

async fn cmd_translate(cfg: &Config, args: TranslateArgs) -> Result<()> {
    let client = SpeechClient::new(cfg)?;
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("reading {}", args.file.display()))?;
    let text = client
        .translate(bytes, &file_name(&args.file), &cfg.asr_model, args.format.as_str())
        .await?;
    println!("{text}");
    Ok(())
}

async fn cmd_realtime(cfg: &Config, globals: &GlobalArgs, args: RealtimeArgs) -> Result<()> {
    let client = SpeechClient::new(cfg)?;
    if !globals.quiet {
        eprintln!(
            "realtime: {}s chunks, device {}, {} -> {}; Ctrl-C to stop",
            args.chunk,
            args.device,
            args.from.as_deref().unwrap_or("auto"),
            args.to
        );
    }
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                if !globals.quiet { eprintln!("stopping"); }
                return Ok(());
            }
            res = realtime_iter(&client, cfg, &args, globals.quiet) => {
                if let Err(e) = res { eprintln!("warn: {e:#}"); }
            }
        }
    }
}

async fn realtime_iter(
    client: &SpeechClient,
    cfg: &Config,
    args: &RealtimeArgs,
    quiet: bool,
) -> Result<()> {
    let (device, secs) = (args.device, args.chunk as f64);
    let pcm = tokio::task::spawn_blocking(move || audio::capture_chunk(device, secs)).await??;
    let mono = tokio::task::spawn_blocking(move || codec::to_asr_mono16(&pcm)).await??;
    if codec::rms_s16(&mono) < SILENCE_RMS {
        return Ok(());
    }
    let wav = codec::wav_mono16(&mono, codec::ASR_RATE);
    let text = translate_chunk(client, cfg, args, wav).await?;
    if text.is_empty() {
        return Ok(());
    }
    println!("[{}] {text}", args.to);
    if args.speak {
        speak_text(client, cfg, &text, quiet).await?;
    }
    Ok(())
}

async fn translate_chunk(
    client: &SpeechClient,
    cfg: &Config,
    args: &RealtimeArgs,
    wav: Vec<u8>,
) -> Result<String> {
    if args.to.eq_ignore_ascii_case("en") {
        return client.translate(wav, "chunk.wav", &cfg.asr_model, "json").await;
    }
    let src = client
        .transcribe(wav, "chunk.wav", &cfg.asr_model, args.from.as_deref(), "json")
        .await?;
    match (&cfg.translate_url, &cfg.translate_model) {
        (Some(url), Some(model)) => client.chat_translate(url, model, &src, &args.to).await,
        _ => Ok(src),
    }
}

async fn speak_text(client: &SpeechClient, cfg: &Config, text: &str, quiet: bool) -> Result<()> {
    let req = SpeakRequest {
        input: text,
        model: &cfg.tts_model,
        voice: &cfg.voice,
        response_format: &cfg.format,
        speed: 1.0,
        language: &cfg.lang,
    };
    let reply = client.speak(&req).await?;
    play_bytes(reply.bytes, reply.content_type, quiet).await
}

fn resolve_text(parts: Vec<String>) -> Result<String> {
    if !parts.is_empty() {
        return Ok(parts.join(" "));
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading text from stdin")?;
    let text = buf.trim().to_owned();
    if text.is_empty() {
        bail!("no text provided: pass arguments or pipe stdin");
    }
    Ok(text)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_owned()
}
