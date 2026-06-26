//! `speak` — a network client for an OpenAI-compatible speech server.
//!
//! Media path: server audio is decoded and resampled with linked `libav*`
//! (ffmpeg-the-third) and played through the native macOS CoreAudio mixer
//! (AVAudioEngine); the microphone is captured natively too. Nothing is
//! shelled out.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};

use speak::client::SpeakRequest;
use speak::config::{Config, GlobalFlags};
use speak::transport::Transport;
use speak::{accel, audio, client, codec, config, daemon, domain, logging, paths};

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
    /// Manage saved voices for cloning.
    Voices {
        #[command(subcommand)]
        action: VoicesAction,
    },
    /// Run or control the persistent-connection daemon.
    Daemon(daemon::DaemonArgs),
    /// Probe the OS and local hardware acceleration libav can use.
    Check,
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
    /// Voice design tags, comma-separated (e.g. "Female, British Accent").
    #[arg(long, value_name = "TAGS")]
    instruct: Option<String>,
    /// Reference transcript when cloning a saved voice.
    #[arg(long, value_name = "TEXT")]
    ref_text: Option<String>,
    /// Target duration hint in seconds.
    #[arg(long, value_name = "SECS")]
    duration: Option<f32>,
    /// Repeatable generation param, key=value (e.g. --set num_step=32).
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
    /// Print the valid voice-design tags and exit.
    #[arg(long)]
    list_designs: bool,
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
    /// Re-voice the source transcript without translating it.
    #[arg(long)]
    repeat: bool,
    /// Synthesize and play the result through the speaker.
    #[arg(long)]
    speak: bool,
    /// Voice design tags for the spoken output (e.g. "Female, British Accent").
    #[arg(long, value_name = "TAGS")]
    instruct: Option<String>,
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

#[derive(Subcommand, Debug)]
enum VoicesAction {
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

#[derive(Args, Debug)]
struct VoiceAddArgs {
    /// Voice name to register.
    #[arg(value_name = "NAME")]
    name: String,
    /// Reference audio file.
    #[arg(long, value_name = "FILE")]
    audio: PathBuf,
    /// Reference transcript for the audio.
    #[arg(long, value_name = "TEXT")]
    ref_text: Option<String>,
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
    let _log_guard = logging::init();
    let cli = Cli::parse();
    run(cli).await
}

async fn run(cli: Cli) -> Result<()> {
    if let Command::Completions { shell } = cli.command {
        return emit_completions(shell);
    }
    let cfg = Config::load(global_flags(&cli.globals))?;
    tracing::info!(host = %cfg.server.host, command = ?std::mem::discriminant(&cli.command), "dispatch");
    match cli.command {
        Command::Check => cmd_check(&cfg),
        Command::Health => cmd_health(&cfg).await,
        Command::Config { action } => cmd_config(action, &cfg),
        Command::Say(args) => cmd_say(&cfg, &cli.globals, args).await,
        Command::Transcribe(args) => cmd_transcribe(&cfg, args).await,
        Command::Translate(args) => cmd_translate(&cfg, args).await,
        Command::Realtime(args) => cmd_realtime(&cfg, &cli.globals, args).await,
        Command::Voices { action } => cmd_voices(&cfg, action).await,
        Command::Daemon(args) => daemon::run(&cfg, args).await,
        Command::Completions { .. } => Ok(()),
    }
}

fn global_flags(g: &GlobalArgs) -> GlobalFlags {
    GlobalFlags {
        host: g.host.clone(),
        api_key: g.api_key.clone(),
        lang: g.lang.clone(),
        voice: g.voice.clone(),
        quiet: g.quiet,
    }
}

fn emit_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "speak", &mut std::io::stdout());
    Ok(())
}

async fn cmd_health(cfg: &Config) -> Result<()> {
    let transport = Transport::connect(cfg).await?;
    let value = transport.proxy("GET", "/health", None).await?.into_json()?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn cmd_check(cfg: &Config) -> Result<()> {
    let report = accel::probe();
    println!("host:                  {}", cfg.server.host);
    println!("os / arch:             {} / {}", report.os, report.arch);
    println!("cpu cores (threading): {}", report.cpu_cores);
    println!("libavcodec:            {}", report.libavcodec);
    println!(
        "hwdevice types:        {}",
        list_or(&report.hwdevice_types, "none")
    );
    println!(
        "audiotoolbox decoders: {}",
        list_or(&report.audiotoolbox_decoders, "none")
    );
    println!(
        "hwaccel policy:        {} (override: {}=auto|off|<decoder>)",
        report.policy,
        accel::ENV_HWACCEL
    );
    println!("config:                {}", paths::config_file().display());
    println!("logs:                  {}", logging::log_dir().display());
    println!("daemon socket:         {}", cfg.daemon.socket.display());
    println!(
        "note: audio decode has no GPU/NVENC path (that hardware is server-side); \
         local acceleration = all-core frame threading + AudioToolbox decoders."
    );
    Ok(())
}

fn list_or(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        empty.to_owned()
    } else {
        items.join(", ")
    }
}

fn cmd_config(action: ConfigAction, cfg: &Config) -> Result<()> {
    let path = paths::config_file();
    match action {
        ConfigAction::Path => println!("{}", path.display()),
        ConfigAction::Show => config_show(cfg),
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

fn config_show(cfg: &Config) {
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

async fn cmd_say(cfg: &Config, globals: &GlobalArgs, args: SayArgs) -> Result<()> {
    if args.list_designs {
        println!("Valid voice-design tags (use with --instruct, comma-separated):");
        for tag in domain::voice_design::CANONICAL_TAGS {
            println!("  {tag}");
        }
        return Ok(());
    }
    let transport = Transport::connect(cfg).await?;
    let text = resolve_text(&args.text)?;
    let format = args
        .format
        .map_or_else(|| cfg.tts.format.clone(), |f| f.as_str().to_owned());
    let reply = synthesize(&transport, cfg, &args, &text, &format).await?;
    if let Some(path) = &args.out {
        let path = resolve_out_path(cfg, path);
        tokio::fs::write(&path, &reply.bytes).await?;
        if !globals.quiet {
            eprintln!("saved {} bytes to {}", reply.bytes.len(), path.display());
        }
    }
    if !globals.quiet {
        report_synth(&reply);
    }
    if !args.no_play && cfg.audio.output.play {
        play_bytes(reply.bytes, reply.content_type, cfg, globals.quiet).await?;
    }
    Ok(())
}

fn resolve_out_path(cfg: &Config, path: &Path) -> PathBuf {
    match (
        &cfg.general.save_dir,
        path.is_absolute() || path.parent().is_some_and(|p| !p.as_os_str().is_empty()),
    ) {
        (Some(dir), false) => dir.join(path),
        _ => path.to_path_buf(),
    }
}

async fn synthesize(
    transport: &Transport,
    cfg: &Config,
    args: &SayArgs,
    text: &str,
    format: &str,
) -> Result<client::AudioReply> {
    if args.native {
        let body =
            serde_json::json!({ "text": text, "language": cfg.tts.language, "speed": args.speed });
        return transport
            .proxy("POST", "/tts", Some(body))
            .await?
            .into_audio();
    }
    let instruct = validate_instruct(args.instruct.as_deref().or(cfg.tts.instruct.as_deref()))?;
    let voice = if instruct.is_some() {
        None
    } else {
        Some(cfg.tts.voice.as_str())
    };
    let req = SpeakRequest {
        input: text,
        model: &cfg.tts.model,
        voice,
        response_format: format,
        speed: args.speed,
        language: &cfg.tts.language,
        instruct: instruct.as_deref(),
        ref_text: args.ref_text.as_deref(),
        duration: args.duration,
        extra: gen_extra(cfg, &args.set)?,
    };
    transport
        .proxy("POST", "/v1/audio/speech", Some(req.to_body()))
        .await?
        .into_audio()
}

/// Validate an optional voice-design instruct string against the canonical tags.
fn validate_instruct(instruct: Option<&str>) -> Result<Option<String>> {
    instruct
        .map(|raw| domain::voice_design::VoiceDesign::parse(raw).map(|d| d.instruct()))
        .transpose()
}

fn report_synth(reply: &client::AudioReply) {
    if let (Some(secs), Some(rtf)) = (&reply.audio_seconds, &reply.rtf) {
        eprintln!("server synthesized {secs}s of audio (RTF {rtf})");
    }
}

/// Merge configured `[tts.gen]` params, then overlay per-call `--set` overrides.
fn gen_extra(cfg: &Config, sets: &[String]) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut map = gen_to_map(&cfg.tts.gen_params);
    for (key, value) in domain::gen_params::parse_overrides(sets)? {
        map.insert(key, value);
    }
    Ok(map)
}

fn gen_to_map(g: &config::Gen) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::json;
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, v: Option<serde_json::Value>| {
        if let Some(v) = v {
            m.insert(k.to_owned(), v);
        }
    };
    put("num_step", g.num_step.map(|v| json!(v)));
    put("guidance_scale", g.guidance_scale.map(|v| json!(v)));
    put("t_shift", g.t_shift.map(|v| json!(v)));
    put(
        "layer_penalty_factor",
        g.layer_penalty_factor.map(|v| json!(v)),
    );
    put(
        "position_temperature",
        g.position_temperature.map(|v| json!(v)),
    );
    put("class_temperature", g.class_temperature.map(|v| json!(v)));
    put("denoise", g.denoise.map(|v| json!(v)));
    put("preprocess_prompt", g.preprocess_prompt.map(|v| json!(v)));
    put("postprocess_output", g.postprocess_output.map(|v| json!(v)));
    put(
        "audio_chunk_duration",
        g.audio_chunk_duration.map(|v| json!(v)),
    );
    put(
        "audio_chunk_threshold",
        g.audio_chunk_threshold.map(|v| json!(v)),
    );
    m
}

async fn cmd_voices(cfg: &Config, action: VoicesAction) -> Result<()> {
    let transport = Transport::connect(cfg).await?;
    match action {
        VoicesAction::List => list_voices(&transport).await?,
        VoicesAction::Add(args) => {
            let bytes = tokio::fs::read(&args.audio)
                .await
                .with_context(|| format!("reading {}", args.audio.display()))?;
            let mut fields = vec![("name".to_owned(), args.name.clone())];
            if let Some(text) = &args.ref_text {
                fields.push(("ref_text".to_owned(), text.clone()));
            }
            let file = Some((bytes, file_name(&args.audio)));
            let msg = transport
                .proxy_multipart("/voices", &fields, file, "audio")
                .await?
                .into_string()?;
            println!("{}", non_empty(msg, &format!("added voice {}", args.name)));
        }
        VoicesAction::Rm { name } => {
            let msg = transport
                .proxy("DELETE", &format!("/voices/{name}"), None)
                .await?
                .into_string()?;
            println!("{}", non_empty(msg, &format!("removed voice {name}")));
        }
    }
    Ok(())
}

async fn list_voices(transport: &Transport) -> Result<()> {
    let value = transport.proxy("GET", "/voices", None).await?.into_json()?;
    let voices: Vec<client::VoiceInfo> = serde_json::from_value(
        value
            .get("voices")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .context("parsing /voices response")?;
    if voices.is_empty() {
        println!("(no saved voices)");
    }
    for v in voices {
        let tag = if v.has_ref_text {
            "  (has ref_text)"
        } else {
            ""
        };
        println!("{}{tag}", v.name);
    }
    Ok(())
}

fn non_empty(text: String, fallback: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

async fn play_bytes(bytes: Vec<u8>, content_type: String, cfg: &Config, quiet: bool) -> Result<()> {
    let opts = codec::DecodeOptions {
        threads: cfg.ffmpeg.threads,
        log_level: cfg.ffmpeg.log_level.clone(),
    };
    let volume = cfg.audio.output.volume;
    let (samples, frames, secs) = tokio::task::spawn_blocking(move || -> Result<_> {
        let pcm = codec::decode(bytes, &opts)?;
        let stats = (pcm.samples.len(), pcm.frames(), pcm.duration_secs());
        audio::play(&pcm, volume)?;
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
    let transport = Transport::connect(cfg).await?;
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("reading {}", args.file.display()))?;
    let fields = asr_fields(
        &cfg.asr.model,
        args.language.as_deref().or(cfg.asr.language.as_deref()),
        args.format.as_str(),
    );
    let file = Some((bytes, file_name(&args.file)));
    let text = transport
        .proxy_multipart("/v1/audio/transcriptions", &fields, file, "file")
        .await?
        .into_text(args.format.as_str())?;
    println!("{text}");
    Ok(())
}

async fn cmd_translate(cfg: &Config, args: TranslateArgs) -> Result<()> {
    let transport = Transport::connect(cfg).await?;
    let bytes = tokio::fs::read(&args.file)
        .await
        .with_context(|| format!("reading {}", args.file.display()))?;
    let fields = asr_fields(&cfg.asr.model, None, args.format.as_str());
    let file = Some((bytes, file_name(&args.file)));
    let text = transport
        .proxy_multipart("/v1/audio/translations", &fields, file, "file")
        .await?
        .into_text(args.format.as_str())?;
    println!("{text}");
    Ok(())
}

fn asr_fields(model: &str, language: Option<&str>, format: &str) -> Vec<client::Field> {
    let mut fields = vec![
        ("model".to_owned(), model.to_owned()),
        ("response_format".to_owned(), format.to_owned()),
    ];
    if let Some(lang) = language {
        fields.push(("language".to_owned(), lang.to_owned()));
    }
    fields
}

async fn cmd_realtime(cfg: &Config, globals: &GlobalArgs, args: RealtimeArgs) -> Result<()> {
    let transport = Transport::connect(cfg).await?;
    let chunk = if args.chunk == 5 {
        cfg.audio.input.chunk_secs
    } else {
        f64::from(args.chunk as u32)
    };
    if !globals.quiet {
        eprintln!(
            "realtime [{}]: {chunk:.0}s chunks, device {}, {} -> {}; Ctrl-C to stop",
            transport.kind(),
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
            res = realtime_iter(&transport, cfg, &args, chunk, globals.quiet) => {
                if let Err(e) = res { eprintln!("warn: {e:#}"); }
            }
        }
    }
}

async fn realtime_iter(
    transport: &Transport,
    cfg: &Config,
    args: &RealtimeArgs,
    chunk: f64,
    quiet: bool,
) -> Result<()> {
    let device = args.device;
    let pcm = tokio::task::spawn_blocking(move || audio::capture_chunk(device, chunk)).await??;
    let mono = tokio::task::spawn_blocking(move || codec::to_asr_mono16(&pcm)).await??;
    if cfg.audio.input.vad && codec::rms_s16(&mono) < silence_floor(cfg) {
        return Ok(());
    }
    let wav = codec::wav_mono16(&mono, codec::ASR_RATE);
    let text = translate_chunk(transport, cfg, args, wav).await?;
    if text.is_empty() {
        return Ok(());
    }
    println!("[{}] {text}", spoken_lang(cfg, args));
    if args.speak {
        speak_text(transport, cfg, args, &text, quiet).await?;
    }
    Ok(())
}

/// Linear RMS floor from the configured silence threshold (dBFS).
fn silence_floor(cfg: &Config) -> f64 {
    10f64.powf(cfg.audio.input.silence_threshold_db / 20.0)
}

async fn transcribe_chunk(
    transport: &Transport,
    cfg: &Config,
    lang: Option<&str>,
    wav: Vec<u8>,
) -> Result<String> {
    let fields = asr_fields(&cfg.asr.model, lang, "json");
    transport
        .proxy_multipart(
            "/v1/audio/transcriptions",
            &fields,
            Some((wav, "chunk.wav".to_owned())),
            "file",
        )
        .await?
        .into_text("json")
}

async fn translate_chunk(
    transport: &Transport,
    cfg: &Config,
    args: &RealtimeArgs,
    wav: Vec<u8>,
) -> Result<String> {
    if args.repeat {
        return transcribe_chunk(transport, cfg, args.from.as_deref(), wav).await;
    }
    if args.to.eq_ignore_ascii_case("en") {
        let fields = asr_fields(&cfg.asr.model, None, "json");
        return transport
            .proxy_multipart(
                "/v1/audio/translations",
                &fields,
                Some((wav, "chunk.wav".to_owned())),
                "file",
            )
            .await?
            .into_text("json");
    }
    let src = transcribe_chunk(transport, cfg, args.from.as_deref(), wav).await?;
    match (&cfg.general.translate_url, &cfg.general.translate_model) {
        (Some(url), Some(model)) => chat_translate(transport, url, model, &src, &args.to).await,
        _ => Ok(src),
    }
}

async fn chat_translate(
    transport: &Transport,
    url: &str,
    model: &str,
    text: &str,
    target: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": format!("Translate the user message into {target}. Reply with only the translation.") },
            { "role": "user", "content": text },
        ],
    });
    let value = transport
        .proxy("POST", url, Some(body))
        .await?
        .into_json()?;
    value
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| anyhow::anyhow!("chat translation missing choices[0].message.content"))
}

/// Language the realtime output is spoken in (target, or source when repeating).
fn spoken_lang<'a>(cfg: &'a Config, args: &'a RealtimeArgs) -> &'a str {
    if args.repeat {
        args.from.as_deref().unwrap_or(cfg.tts.language.as_str())
    } else {
        args.to.as_str()
    }
}

async fn speak_text(
    transport: &Transport,
    cfg: &Config,
    args: &RealtimeArgs,
    text: &str,
    quiet: bool,
) -> Result<()> {
    let instruct = validate_instruct(args.instruct.as_deref().or(cfg.tts.instruct.as_deref()))?;
    let voice = if instruct.is_some() {
        None
    } else {
        Some(cfg.tts.voice.as_str())
    };
    let req = SpeakRequest {
        input: text,
        model: &cfg.tts.model,
        voice,
        response_format: &cfg.tts.format,
        speed: 1.0,
        language: spoken_lang(cfg, args),
        instruct: instruct.as_deref(),
        ref_text: None,
        duration: None,
        extra: gen_to_map(&cfg.tts.gen_params),
    };
    let reply = transport
        .proxy("POST", "/v1/audio/speech", Some(req.to_body()))
        .await?
        .into_audio()?;
    play_bytes(reply.bytes, reply.content_type, cfg, quiet).await
}

fn resolve_text(parts: &[String]) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asr_fields_omit_language_when_none() {
        // Translate (always English) sends no language hint.
        let fields = asr_fields("whisper-1", None, "json");
        assert_eq!(
            fields,
            vec![
                ("model".to_owned(), "whisper-1".to_owned()),
                ("response_format".to_owned(), "json".to_owned()),
            ]
        );
    }

    #[test]
    fn asr_fields_include_language_when_present() {
        // Transcribe forwards the source-language hint.
        let fields = asr_fields("whisper-1", Some("pt"), "text");
        assert!(fields.contains(&("language".to_owned(), "pt".to_owned())));
        assert_eq!(fields.len(), 3);
    }

    #[test]
    fn gen_to_map_only_emits_set_params() {
        let params = config::Gen {
            num_step: Some(24),
            guidance_scale: Some(3.0),
            denoise: Some(true),
            ..config::Gen::default()
        };
        let map = gen_to_map(&params);
        assert_eq!(map.get("num_step"), Some(&serde_json::json!(24)));
        assert_eq!(map.get("guidance_scale"), Some(&serde_json::json!(3.0)));
        assert_eq!(map.get("denoise"), Some(&serde_json::json!(true)));
        assert!(!map.contains_key("t_shift"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn gen_to_map_empty_when_nothing_set() {
        assert!(gen_to_map(&config::Gen::default()).is_empty());
    }

    #[test]
    fn validate_instruct_accepts_canonical_tags() {
        let out = validate_instruct(Some("Female, British Accent")).unwrap();
        assert_eq!(out.as_deref(), Some("Female, British Accent"));
    }

    #[test]
    fn validate_instruct_passes_none_through() {
        assert!(validate_instruct(None).unwrap().is_none());
    }

    #[test]
    fn validate_instruct_rejects_free_text() {
        assert!(validate_instruct(Some("sounds friendly")).is_err());
    }

    #[test]
    fn resolve_text_joins_positional_args() {
        let parts = vec!["hello".to_owned(), "world".to_owned()];
        assert_eq!(resolve_text(&parts).unwrap(), "hello world");
    }

    #[test]
    fn file_name_extracts_basename_with_fallback() {
        assert_eq!(file_name(Path::new("/a/b/clip.wav")), "clip.wav");
        assert_eq!(file_name(Path::new("plain.mp3")), "plain.mp3");
        assert_eq!(file_name(Path::new("/")), "audio");
    }

    #[test]
    fn non_empty_falls_back_on_blank() {
        assert_eq!(non_empty("  done  ".to_owned(), "fallback"), "done");
        assert_eq!(non_empty("   ".to_owned(), "fallback"), "fallback");
    }

    #[test]
    fn list_or_renders_items_or_placeholder() {
        assert_eq!(list_or(&["a".to_owned(), "b".to_owned()], "none"), "a, b");
        assert_eq!(list_or(&[], "none"), "none");
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
    fn cli_definition_is_valid() {
        // Guards against clap derive misconfiguration (duplicate args, etc.).
        Cli::command().debug_assert();
    }
}
