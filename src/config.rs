//! Full configuration catalog with strict precedence per key:
//! **flag > env (`SPEAK_*`) > `~/.speak/config.toml` > built-in default**.
//!
//! Every value records its [`Origin`] so `config show` can report where each
//! setting came from. The file lives at [`crate::paths::config_file`]; the
//! legacy `~/.config/speak/config.toml` is read as a one-time fallback.

use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::paths;

/// Default server base URL.
pub const DEFAULT_HOST: &str = "http://solaris:8800";
/// Default language.
pub const DEFAULT_LANG: &str = "pt-BR";
/// Default voice.
pub const DEFAULT_VOICE: &str = "alloy";
/// Default audio response format.
pub const DEFAULT_FORMAT: &str = "mp3";

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A command-line flag.
    Flag,
    /// An `SPEAK_*` environment variable.
    Env,
    /// The TOML config file.
    Toml,
    /// The built-in default.
    Default,
}

impl Display for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Flag => "flag",
            Self::Env => "env",
            Self::Toml => "toml",
            Self::Default => "default",
        })
    }
}

/// Global flags that participate in resolution (highest precedence).
#[derive(Debug, Default, Clone)]
pub struct GlobalFlags {
    /// `--host`.
    pub host: Option<String>,
    /// `--api-key`.
    pub api_key: Option<String>,
    /// `--lang`.
    pub lang: Option<String>,
    /// `--voice`.
    pub voice: Option<String>,
    /// `-q/--quiet`.
    pub quiet: bool,
}

/// `[server]` HTTP/client settings.
#[derive(Debug, Clone)]
pub struct Server {
    /// Base URL.
    pub host: String,
    /// Bearer key.
    pub api_key: Option<String>,
    /// Per-request timeout (seconds).
    pub timeout: u64,
    /// Connect timeout (seconds).
    pub connect_timeout: u64,
    /// Idle connections kept per host.
    pub pool_max_idle: usize,
    /// Idle connection lifetime (seconds).
    pub pool_idle_timeout: u64,
    /// TCP keep-alive (seconds).
    pub tcp_keepalive: u64,
    /// Prefer HTTP/2 prior knowledge.
    pub http2: bool,
    /// User-Agent header.
    pub user_agent: String,
}

/// `[tts.gen]` pass-through generation params (unset => server default).
#[derive(Debug, Clone, Default)]
pub struct Gen {
    /// Diffusion steps.
    pub num_step: Option<i64>,
    /// Classifier-free guidance scale.
    pub guidance_scale: Option<f64>,
    /// Time shift.
    pub t_shift: Option<f64>,
    /// Layer penalty factor.
    pub layer_penalty_factor: Option<f64>,
    /// Position temperature.
    pub position_temperature: Option<f64>,
    /// Class temperature.
    pub class_temperature: Option<f64>,
    /// Denoise toggle.
    pub denoise: Option<bool>,
    /// Preprocess prompt toggle.
    pub preprocess_prompt: Option<bool>,
    /// Postprocess output toggle.
    pub postprocess_output: Option<bool>,
    /// Audio chunk duration.
    pub audio_chunk_duration: Option<f64>,
    /// Audio chunk threshold.
    pub audio_chunk_threshold: Option<f64>,
}

/// `[tts]` synthesis settings.
#[derive(Debug, Clone)]
pub struct Tts {
    /// Language hint.
    pub language: String,
    /// Voice (saved name for cloning).
    pub voice: String,
    /// Response format.
    pub format: String,
    /// Model id.
    pub model: String,
    /// Speed multiplier.
    pub speed: f32,
    /// Voice-design tags.
    pub instruct: Option<String>,
    /// Use native `/tts`.
    pub native: bool,
    /// Generation params.
    pub gen_params: Gen,
}

/// `[asr]` recognition settings.
#[derive(Debug, Clone)]
pub struct Asr {
    /// Model id.
    pub model: String,
    /// Language hint.
    pub language: Option<String>,
    /// Output format.
    pub format: String,
}

/// `[audio.output]` playback settings.
#[derive(Debug, Clone)]
pub struct Output {
    /// Output device hint (advisory; default = system default).
    pub device: Option<String>,
    /// Mixer output volume `0.0..=1.0`.
    pub volume: f32,
    /// Forced output sample rate (advisory).
    pub sample_rate: Option<u32>,
    /// Forced channel count (advisory).
    pub channels: Option<u16>,
    /// Scheduler buffer frames (advisory).
    pub buffer_frames: Option<u32>,
    /// Whether to play locally by default.
    pub play: bool,
}

/// `[audio.input]` capture settings.
#[derive(Debug, Clone)]
pub struct Input {
    /// Input device index.
    pub device: u32,
    /// Capture sample rate (advisory; ASR resamples to 16 kHz).
    pub sample_rate: u32,
    /// Capture channels (advisory).
    pub channels: u16,
    /// Chunk length seconds.
    pub chunk_secs: f64,
    /// Silence gate threshold in dBFS.
    pub silence_threshold_db: f64,
    /// Enable the silence/VAD gate.
    pub vad: bool,
}

/// `[audio]` container.
#[derive(Debug, Clone)]
pub struct Audio {
    /// Output settings.
    pub output: Output,
    /// Input settings.
    pub input: Input,
}

/// `[ffmpeg]` codec/resampler settings.
#[derive(Debug, Clone)]
pub struct Ffmpeg {
    /// Decoder thread count (0 = all cores).
    pub threads: u32,
    /// Resampler engine (`swr` or `soxr`).
    pub resampler: String,
    /// Resampler quality / filter size (engine-specific).
    pub resample_quality: Option<i64>,
    /// Enable output dither (for integer formats).
    pub dither: bool,
    /// Forced intermediate sample format (advisory).
    pub sample_fmt: Option<String>,
    /// libav log level.
    pub log_level: String,
    /// Extra libavfilter audio filtergraph applied before playback.
    pub extra_filters: Option<String>,
}

/// `[realtime]` loop defaults.
#[derive(Debug, Clone)]
pub struct Realtime {
    /// Source language.
    pub from: Option<String>,
    /// Target language.
    pub to: String,
    /// Speak the result.
    pub speak: bool,
    /// Chunk length seconds.
    pub chunk_secs: f64,
}

/// `[daemon]` settings.
#[derive(Debug, Clone)]
pub struct Daemon {
    /// Unix socket path.
    pub socket: PathBuf,
    /// Idle shutdown timeout seconds (0 = never).
    pub idle_timeout: u64,
    /// Auto-start a daemon for CLI calls when none is running.
    pub autostart: bool,
}

/// `[general]` and misc settings.
#[derive(Debug, Clone)]
pub struct General {
    /// Suppress status output.
    pub quiet: bool,
    /// Prefer JSON output where applicable.
    pub json: bool,
    /// Allow ANSI colour.
    pub color: bool,
    /// Temp directory.
    pub temp_dir: Option<PathBuf>,
    /// Log level filter.
    pub log: Option<String>,
}

/// `[http]` networking endpoints + output paths (T037, ADR-0006).
///
/// Splits the chat-MT translation endpoint and the default `-o` save directory
/// out of `[general]` into their own section. Each key keeps its `SPEAK_*` env
/// override and code default — no hardcoded tunables (FR-18) — and migrates
/// transparently from the legacy `[general]` location (a file that still carries
/// them under `[general]` keeps working; `config show` records the origin).
#[derive(Debug, Clone)]
pub struct Http {
    /// Chat-completions translation endpoint (chat-MT Strategy, T039).
    pub translate_url: Option<String>,
    /// Chat translation model.
    pub translate_model: Option<String>,
    /// Default save directory for `say -o`.
    pub save_dir: Option<PathBuf>,
}

/// `[retry]` resilience policy — the TOML projection of the domain
/// [`crate::domain::retry::RetryPolicy`] value object (FR-17, ADR-0004). Every
/// field is env-overridable so there are no hardcoded magic numbers (FR-18).
#[derive(Debug, Clone)]
pub struct Retry {
    /// The resolved backoff/jitter/classification policy.
    pub policy: crate::domain::retry::RetryPolicy,
    /// Optional fixed jitter seed (unset => OS entropy) for reproducible runs.
    pub jitter_seed: Option<u64>,
}

/// Fully-resolved configuration with per-key origins.
#[derive(Debug, Clone)]
pub struct Config {
    /// Server section.
    pub server: Server,
    /// TTS section.
    pub tts: Tts,
    /// ASR section.
    pub asr: Asr,
    /// Audio section.
    pub audio: Audio,
    /// ffmpeg section.
    pub ffmpeg: Ffmpeg,
    /// Realtime section.
    pub realtime: Realtime,
    /// Daemon section.
    pub daemon: Daemon,
    /// General section.
    pub general: General,
    /// HTTP endpoints + output paths section.
    pub http: Http,
    /// Retry/backoff resilience policy.
    pub retry: Retry,
    entries: Vec<(String, String, Origin)>,
}

impl Config {
    /// Resolve the full configuration from flags + env + file + defaults.
    pub fn load(flags: GlobalFlags) -> Result<Self> {
        let file = load_file()?;
        Resolver::new(flags, file).finish()
    }

    /// Ordered (key, rendered-value, origin) entries for `config show`.
    #[must_use]
    pub fn entries(&self) -> &[(String, String, Origin)] {
        &self.entries
    }
}

// --------------------------------------------------------------------------
// File model
// --------------------------------------------------------------------------

/// On-disk config; all fields optional, mirrors the resolved sections.
#[derive(Debug, Default, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    server: FileServer,
    #[serde(default)]
    tts: FileTts,
    #[serde(default)]
    asr: FileAsr,
    #[serde(default)]
    audio: FileAudio,
    #[serde(default)]
    ffmpeg: FileFfmpeg,
    #[serde(default)]
    realtime: FileRealtime,
    #[serde(default)]
    daemon: FileDaemon,
    #[serde(default)]
    general: FileGeneral,
    #[serde(default)]
    http: FileHttp,
    #[serde(default)]
    retry: FileRetry,
}

#[derive(Debug, Default, Deserialize)]
struct FileServer {
    host: Option<String>,
    api_key: Option<String>,
    timeout: Option<u64>,
    connect_timeout: Option<u64>,
    pool_max_idle: Option<usize>,
    pool_idle_timeout: Option<u64>,
    tcp_keepalive: Option<u64>,
    http2: Option<bool>,
    user_agent: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileGen {
    num_step: Option<i64>,
    guidance_scale: Option<f64>,
    t_shift: Option<f64>,
    layer_penalty_factor: Option<f64>,
    position_temperature: Option<f64>,
    class_temperature: Option<f64>,
    denoise: Option<bool>,
    preprocess_prompt: Option<bool>,
    postprocess_output: Option<bool>,
    audio_chunk_duration: Option<f64>,
    audio_chunk_threshold: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct FileTts {
    language: Option<String>,
    voice: Option<String>,
    format: Option<String>,
    model: Option<String>,
    speed: Option<f32>,
    instruct: Option<String>,
    native: Option<bool>,
    #[serde(default, rename = "gen")]
    gen_params: FileGen,
}

#[derive(Debug, Default, Deserialize)]
struct FileAsr {
    model: Option<String>,
    language: Option<String>,
    format: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileOutput {
    device: Option<String>,
    volume: Option<f32>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    buffer_frames: Option<u32>,
    play: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct FileInput {
    device: Option<u32>,
    sample_rate: Option<u32>,
    channels: Option<u16>,
    chunk_secs: Option<f64>,
    silence_threshold_db: Option<f64>,
    vad: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct FileAudio {
    #[serde(default)]
    output: FileOutput,
    #[serde(default)]
    input: FileInput,
}

#[derive(Debug, Default, Deserialize)]
struct FileFfmpeg {
    threads: Option<u32>,
    resampler: Option<String>,
    resample_quality: Option<i64>,
    dither: Option<bool>,
    sample_fmt: Option<String>,
    log_level: Option<String>,
    extra_filters: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileRealtime {
    from: Option<String>,
    to: Option<String>,
    speak: Option<bool>,
    chunk_secs: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct FileDaemon {
    socket: Option<String>,
    idle_timeout: Option<u64>,
    autostart: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct FileGeneral {
    quiet: Option<bool>,
    json: Option<bool>,
    color: Option<bool>,
    temp_dir: Option<String>,
    log: Option<String>,
    // Legacy `[general]` location for the keys that moved to `[http]`; retained
    // so an unmigrated config file keeps resolving (see `Resolver::http`).
    save_dir: Option<String>,
    translate_url: Option<String>,
    translate_model: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileHttp {
    translate_url: Option<String>,
    translate_model: Option<String>,
    save_dir: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FileRetry {
    max_retries: Option<u32>,
    backoff_initial_ms: Option<u64>,
    backoff_max_ms: Option<u64>,
    multiplier: Option<f64>,
    jitter: Option<bool>,
    jitter_seed: Option<u64>,
    retry_on: Option<Vec<String>>,
}

/// Load the TOML config, falling back to the legacy path, else empty.
pub fn load_file() -> Result<FileConfig> {
    let path = paths::config_file();
    let chosen = if path.exists() {
        path
    } else {
        let legacy = paths::legacy_config_file();
        if legacy.exists() {
            legacy
        } else {
            return Ok(FileConfig::default());
        }
    };
    let text = std::fs::read_to_string(&chosen)
        .with_context(|| format!("reading config {}", chosen.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing config {}", chosen.display()))
}

// --------------------------------------------------------------------------
// Resolver
// --------------------------------------------------------------------------

struct Resolver {
    flags: GlobalFlags,
    file: FileConfig,
    entries: Vec<(String, String, Origin)>,
}

fn env_parse<T: FromStr>(name: &str) -> Option<T> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn pick_req<T: FromStr + Clone>(
    flag: Option<T>,
    env: &str,
    toml: Option<T>,
    default: T,
) -> (T, Origin) {
    if let Some(v) = flag {
        return (v, Origin::Flag);
    }
    if let Some(v) = env_parse::<T>(env) {
        return (v, Origin::Env);
    }
    if let Some(v) = toml {
        return (v, Origin::Toml);
    }
    (default, Origin::Default)
}

fn pick_opt<T: FromStr + Clone>(
    flag: Option<T>,
    env: &str,
    toml: Option<T>,
) -> (Option<T>, Origin) {
    if let Some(v) = flag {
        return (Some(v), Origin::Flag);
    }
    if let Some(v) = env_parse::<T>(env) {
        return (Some(v), Origin::Env);
    }
    if let Some(v) = toml {
        return (Some(v), Origin::Toml);
    }
    (None, Origin::Default)
}

fn flag_true(set: bool) -> Option<bool> {
    if set { Some(true) } else { None }
}

fn default_user_agent() -> String {
    concat!("speak/", env!("CARGO_PKG_VERSION")).to_owned()
}

impl Resolver {
    fn new(flags: GlobalFlags, file: FileConfig) -> Self {
        Self {
            flags,
            file,
            entries: Vec::new(),
        }
    }

    fn record(&mut self, key: &str, value: String, origin: Origin) {
        self.entries.push((key.to_owned(), value, origin));
    }

    fn val<T>(&mut self, key: &str, flag: Option<T>, env: &str, toml: Option<T>, default: T) -> T
    where
        T: FromStr + Display + Clone,
    {
        let (value, origin) = pick_req(flag, env, toml, default);
        self.record(key, value.to_string(), origin);
        value
    }

    fn opt<T>(&mut self, key: &str, flag: Option<T>, env: &str, toml: Option<T>) -> Option<T>
    where
        T: FromStr + Display + Clone,
    {
        let (value, origin) = pick_opt(flag, env, toml);
        let shown = value
            .as_ref()
            .map_or_else(|| "unset".to_owned(), ToString::to_string);
        self.record(key, shown, origin);
        value
    }

    fn secret(
        &mut self,
        key: &str,
        flag: Option<String>,
        env: &str,
        toml: Option<String>,
    ) -> Option<String> {
        let (value, origin) = pick_opt(flag, env, toml);
        self.record(
            key,
            if value.is_some() { "***" } else { "unset" }.to_owned(),
            origin,
        );
        value
    }

    fn finish(mut self) -> Result<Config> {
        let server = self.server();
        let tts = self.tts();
        let asr = self.asr();
        let audio = self.audio();
        let ffmpeg = self.ffmpeg();
        let realtime = self.realtime();
        let daemon = self.daemon();
        let general = self.general();
        let http = self.http();
        let retry = self.retry();
        Ok(Config {
            server,
            tts,
            asr,
            audio,
            ffmpeg,
            realtime,
            daemon,
            general,
            http,
            retry,
            entries: self.entries,
        })
    }

    fn server(&mut self) -> Server {
        Server {
            host: self.val(
                "server.host",
                self.flags.host.clone(),
                "SPEAK_HOST",
                self.file.server.host.clone(),
                DEFAULT_HOST.to_owned(),
            ),
            api_key: self.secret(
                "server.api_key",
                self.flags.api_key.clone(),
                "SPEAK_API_KEY",
                self.file.server.api_key.clone(),
            ),
            timeout: self.val(
                "server.timeout",
                None,
                "SPEAK_SERVER_TIMEOUT",
                self.file.server.timeout,
                300,
            ),
            connect_timeout: self.val(
                "server.connect_timeout",
                None,
                "SPEAK_SERVER_CONNECT_TIMEOUT",
                self.file.server.connect_timeout,
                10,
            ),
            pool_max_idle: self.val(
                "server.pool_max_idle",
                None,
                "SPEAK_SERVER_POOL_MAX_IDLE",
                self.file.server.pool_max_idle,
                8,
            ),
            pool_idle_timeout: self.val(
                "server.pool_idle_timeout",
                None,
                "SPEAK_SERVER_POOL_IDLE_TIMEOUT",
                self.file.server.pool_idle_timeout,
                90,
            ),
            tcp_keepalive: self.val(
                "server.tcp_keepalive",
                None,
                "SPEAK_SERVER_TCP_KEEPALIVE",
                self.file.server.tcp_keepalive,
                60,
            ),
            http2: self.val(
                "server.http2",
                None,
                "SPEAK_SERVER_HTTP2",
                self.file.server.http2,
                false,
            ),
            user_agent: self.val(
                "server.user_agent",
                None,
                "SPEAK_SERVER_USER_AGENT",
                self.file.server.user_agent.clone(),
                default_user_agent(),
            ),
        }
    }

    fn tts(&mut self) -> Tts {
        Tts {
            language: self.val(
                "tts.language",
                self.flags.lang.clone(),
                "SPEAK_LANG",
                self.file.tts.language.clone(),
                DEFAULT_LANG.to_owned(),
            ),
            voice: self.val(
                "tts.voice",
                self.flags.voice.clone(),
                "SPEAK_VOICE",
                self.file.tts.voice.clone(),
                DEFAULT_VOICE.to_owned(),
            ),
            format: self.val(
                "tts.format",
                None,
                "SPEAK_FORMAT",
                self.file.tts.format.clone(),
                DEFAULT_FORMAT.to_owned(),
            ),
            model: self.val(
                "tts.model",
                None,
                "SPEAK_TTS_MODEL",
                self.file.tts.model.clone(),
                "tts-1".to_owned(),
            ),
            speed: self.val(
                "tts.speed",
                None,
                "SPEAK_TTS_SPEED",
                self.file.tts.speed,
                1.0,
            ),
            instruct: self.opt(
                "tts.instruct",
                None,
                "SPEAK_TTS_INSTRUCT",
                self.file.tts.instruct.clone(),
            ),
            native: self.val(
                "tts.native",
                None,
                "SPEAK_TTS_NATIVE",
                self.file.tts.native,
                false,
            ),
            gen_params: self.gen_params(),
        }
    }

    fn gen_params(&mut self) -> Gen {
        Gen {
            num_step: self.opt(
                "tts.gen.num_step",
                None,
                "SPEAK_TTS_GEN_NUM_STEP",
                self.file.tts.gen_params.num_step,
            ),
            guidance_scale: self.opt(
                "tts.gen.guidance_scale",
                None,
                "SPEAK_TTS_GEN_GUIDANCE_SCALE",
                self.file.tts.gen_params.guidance_scale,
            ),
            t_shift: self.opt(
                "tts.gen.t_shift",
                None,
                "SPEAK_TTS_GEN_T_SHIFT",
                self.file.tts.gen_params.t_shift,
            ),
            layer_penalty_factor: self.opt(
                "tts.gen.layer_penalty_factor",
                None,
                "SPEAK_TTS_GEN_LAYER_PENALTY_FACTOR",
                self.file.tts.gen_params.layer_penalty_factor,
            ),
            position_temperature: self.opt(
                "tts.gen.position_temperature",
                None,
                "SPEAK_TTS_GEN_POSITION_TEMPERATURE",
                self.file.tts.gen_params.position_temperature,
            ),
            class_temperature: self.opt(
                "tts.gen.class_temperature",
                None,
                "SPEAK_TTS_GEN_CLASS_TEMPERATURE",
                self.file.tts.gen_params.class_temperature,
            ),
            denoise: self.opt(
                "tts.gen.denoise",
                None,
                "SPEAK_TTS_GEN_DENOISE",
                self.file.tts.gen_params.denoise,
            ),
            preprocess_prompt: self.opt(
                "tts.gen.preprocess_prompt",
                None,
                "SPEAK_TTS_GEN_PREPROCESS_PROMPT",
                self.file.tts.gen_params.preprocess_prompt,
            ),
            postprocess_output: self.opt(
                "tts.gen.postprocess_output",
                None,
                "SPEAK_TTS_GEN_POSTPROCESS_OUTPUT",
                self.file.tts.gen_params.postprocess_output,
            ),
            audio_chunk_duration: self.opt(
                "tts.gen.audio_chunk_duration",
                None,
                "SPEAK_TTS_GEN_AUDIO_CHUNK_DURATION",
                self.file.tts.gen_params.audio_chunk_duration,
            ),
            audio_chunk_threshold: self.opt(
                "tts.gen.audio_chunk_threshold",
                None,
                "SPEAK_TTS_GEN_AUDIO_CHUNK_THRESHOLD",
                self.file.tts.gen_params.audio_chunk_threshold,
            ),
        }
    }

    fn asr(&mut self) -> Asr {
        Asr {
            model: self.val(
                "asr.model",
                None,
                "SPEAK_ASR_MODEL",
                self.file.asr.model.clone(),
                "whisper-1".to_owned(),
            ),
            language: self.opt(
                "asr.language",
                None,
                "SPEAK_ASR_LANGUAGE",
                self.file.asr.language.clone(),
            ),
            format: self.val(
                "asr.format",
                None,
                "SPEAK_ASR_FORMAT",
                self.file.asr.format.clone(),
                "json".to_owned(),
            ),
        }
    }

    fn audio(&mut self) -> Audio {
        Audio {
            output: self.output(),
            input: self.input(),
        }
    }

    fn output(&mut self) -> Output {
        Output {
            device: self.opt(
                "audio.output.device",
                None,
                "SPEAK_AUDIO_OUTPUT_DEVICE",
                self.file.audio.output.device.clone(),
            ),
            volume: self.val(
                "audio.output.volume",
                None,
                "SPEAK_AUDIO_OUTPUT_VOLUME",
                self.file.audio.output.volume,
                1.0,
            ),
            sample_rate: self.opt(
                "audio.output.sample_rate",
                None,
                "SPEAK_AUDIO_OUTPUT_SAMPLE_RATE",
                self.file.audio.output.sample_rate,
            ),
            channels: self.opt(
                "audio.output.channels",
                None,
                "SPEAK_AUDIO_OUTPUT_CHANNELS",
                self.file.audio.output.channels,
            ),
            buffer_frames: self.opt(
                "audio.output.buffer_frames",
                None,
                "SPEAK_AUDIO_OUTPUT_BUFFER_FRAMES",
                self.file.audio.output.buffer_frames,
            ),
            play: self.val(
                "audio.output.play",
                None,
                "SPEAK_AUDIO_OUTPUT_PLAY",
                self.file.audio.output.play,
                true,
            ),
        }
    }

    fn input(&mut self) -> Input {
        Input {
            device: self.val(
                "audio.input.device",
                None,
                "SPEAK_AUDIO_INPUT_DEVICE",
                self.file.audio.input.device,
                0,
            ),
            sample_rate: self.val(
                "audio.input.sample_rate",
                None,
                "SPEAK_AUDIO_INPUT_SAMPLE_RATE",
                self.file.audio.input.sample_rate,
                16_000,
            ),
            channels: self.val(
                "audio.input.channels",
                None,
                "SPEAK_AUDIO_INPUT_CHANNELS",
                self.file.audio.input.channels,
                1,
            ),
            chunk_secs: self.val(
                "audio.input.chunk_secs",
                None,
                "SPEAK_AUDIO_INPUT_CHUNK_SECS",
                self.file.audio.input.chunk_secs,
                5.0,
            ),
            silence_threshold_db: self.val(
                "audio.input.silence_threshold_db",
                None,
                "SPEAK_AUDIO_INPUT_SILENCE_THRESHOLD_DB",
                self.file.audio.input.silence_threshold_db,
                -38.0,
            ),
            vad: self.val(
                "audio.input.vad",
                None,
                "SPEAK_AUDIO_INPUT_VAD",
                self.file.audio.input.vad,
                true,
            ),
        }
    }

    fn ffmpeg(&mut self) -> Ffmpeg {
        Ffmpeg {
            threads: self.val(
                "ffmpeg.threads",
                None,
                "SPEAK_FFMPEG_THREADS",
                self.file.ffmpeg.threads,
                0,
            ),
            resampler: self.val(
                "ffmpeg.resampler",
                None,
                "SPEAK_FFMPEG_RESAMPLER",
                self.file.ffmpeg.resampler.clone(),
                "swr".to_owned(),
            ),
            resample_quality: self.opt(
                "ffmpeg.resample_quality",
                None,
                "SPEAK_FFMPEG_RESAMPLE_QUALITY",
                self.file.ffmpeg.resample_quality,
            ),
            dither: self.val(
                "ffmpeg.dither",
                None,
                "SPEAK_FFMPEG_DITHER",
                self.file.ffmpeg.dither,
                true,
            ),
            sample_fmt: self.opt(
                "ffmpeg.sample_fmt",
                None,
                "SPEAK_FFMPEG_SAMPLE_FMT",
                self.file.ffmpeg.sample_fmt.clone(),
            ),
            log_level: self.val(
                "ffmpeg.log_level",
                None,
                "SPEAK_FFMPEG_LOG_LEVEL",
                self.file.ffmpeg.log_level.clone(),
                "error".to_owned(),
            ),
            extra_filters: self.opt(
                "ffmpeg.extra_filters",
                None,
                "SPEAK_FFMPEG_EXTRA_FILTERS",
                self.file.ffmpeg.extra_filters.clone(),
            ),
        }
    }

    fn realtime(&mut self) -> Realtime {
        Realtime {
            from: self.opt(
                "realtime.from",
                None,
                "SPEAK_REALTIME_FROM",
                self.file.realtime.from.clone(),
            ),
            to: self.val(
                "realtime.to",
                None,
                "SPEAK_REALTIME_TO",
                self.file.realtime.to.clone(),
                "en".to_owned(),
            ),
            speak: self.val(
                "realtime.speak",
                None,
                "SPEAK_REALTIME_SPEAK",
                self.file.realtime.speak,
                false,
            ),
            chunk_secs: self.val(
                "realtime.chunk_secs",
                None,
                "SPEAK_REALTIME_CHUNK_SECS",
                self.file.realtime.chunk_secs,
                5.0,
            ),
        }
    }

    fn daemon(&mut self) -> Daemon {
        let default_socket = paths::default_socket().display().to_string();
        let socket = self.val(
            "daemon.socket",
            None,
            "SPEAK_DAEMON_SOCKET",
            self.file.daemon.socket.clone(),
            default_socket,
        );
        Daemon {
            socket: PathBuf::from(socket),
            idle_timeout: self.val(
                "daemon.idle_timeout",
                None,
                "SPEAK_DAEMON_IDLE_TIMEOUT",
                self.file.daemon.idle_timeout,
                0,
            ),
            autostart: self.val(
                "daemon.autostart",
                None,
                "SPEAK_DAEMON_AUTOSTART",
                self.file.daemon.autostart,
                false,
            ),
        }
    }

    fn general(&mut self) -> General {
        self.record_config_path();
        General {
            quiet: self.val(
                "general.quiet",
                flag_true(self.flags.quiet),
                "SPEAK_QUIET",
                self.file.general.quiet,
                false,
            ),
            json: self.val(
                "general.json",
                None,
                "SPEAK_JSON",
                self.file.general.json,
                false,
            ),
            color: self.val(
                "general.color",
                None,
                "SPEAK_COLOR",
                self.file.general.color,
                true,
            ),
            temp_dir: self
                .opt(
                    "general.temp_dir",
                    None,
                    "SPEAK_TEMP_DIR",
                    self.file.general.temp_dir.clone(),
                )
                .map(PathBuf::from),
            log: self.opt(
                "general.log",
                None,
                "SPEAK_LOG",
                self.file.general.log.clone(),
            ),
        }
    }

    /// Resolve `[http]`, migrating each key from the legacy `[general]` location
    /// when the file has not been re-sectioned yet (the `[http]` value wins).
    fn http(&mut self) -> Http {
        let translate_url = self.opt(
            "http.translate_url",
            None,
            "SPEAK_TRANSLATE_URL",
            self.file
                .http
                .translate_url
                .clone()
                .or_else(|| self.file.general.translate_url.clone()),
        );
        let translate_model = self.opt(
            "http.translate_model",
            None,
            "SPEAK_TRANSLATE_MODEL",
            self.file
                .http
                .translate_model
                .clone()
                .or_else(|| self.file.general.translate_model.clone()),
        );
        let save_dir = self
            .opt(
                "http.save_dir",
                None,
                "SPEAK_SAVE_DIR",
                self.file
                    .http
                    .save_dir
                    .clone()
                    .or_else(|| self.file.general.save_dir.clone()),
            )
            .map(PathBuf::from);
        Http {
            translate_url,
            translate_model,
            save_dir,
        }
    }

    fn record_config_path(&mut self) {
        let origin = if std::env::var_os("SPEAK_CONFIG").is_some() {
            Origin::Env
        } else {
            Origin::Default
        };
        self.record(
            "general.config_path",
            paths::config_file().display().to_string(),
            origin,
        );
    }

    fn retry(&mut self) -> Retry {
        use crate::domain::retry::RetryPolicy;
        let policy = RetryPolicy {
            max_retries: self.val(
                "retry.max_retries",
                None,
                "SPEAK_RETRY_MAX",
                self.file.retry.max_retries,
                3,
            ),
            backoff_initial_ms: self.val(
                "retry.backoff_initial_ms",
                None,
                "SPEAK_RETRY_BACKOFF_MS",
                self.file.retry.backoff_initial_ms,
                200,
            ),
            backoff_max_ms: self.val(
                "retry.backoff_max_ms",
                None,
                "SPEAK_RETRY_BACKOFF_MAX_MS",
                self.file.retry.backoff_max_ms,
                5_000,
            ),
            multiplier: self.val(
                "retry.multiplier",
                None,
                "SPEAK_RETRY_MULTIPLIER",
                self.file.retry.multiplier,
                2.0,
            ),
            jitter: self.val(
                "retry.jitter",
                None,
                "SPEAK_RETRY_JITTER",
                self.file.retry.jitter,
                true,
            ),
            retry_on: self.retry_on(),
        };
        let jitter_seed = self.opt(
            "retry.jitter_seed",
            None,
            "SPEAK_RETRY_JITTER_SEED",
            self.file.retry.jitter_seed,
        );
        Retry {
            policy,
            jitter_seed,
        }
    }

    fn retry_on(&mut self) -> crate::domain::retry::RetryOn {
        let file_spec = self.file.retry.retry_on.as_ref().map(|v| v.join("+"));
        let spec = self.val(
            "retry.retry_on",
            None,
            "SPEAK_RETRY_ON",
            file_spec,
            "connect+timeout+5xx+429".to_owned(),
        );
        crate::domain::retry::RetryOn::parse(&spec)
    }
}

/// Render a fully-commented default config file body.
#[must_use]
pub fn default_file_toml() -> String {
    include_str!("config_template.toml").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testenv::ENV_LOCK;

    /// Run `body` with `name` set to `value`, restoring the prior value after.
    fn with_env<T>(name: &str, value: &str, body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(name).ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var(name, value) };
        let out = body();
        match prev {
            // TODO: Audit that the environment access only happens in single-threaded code.
            Some(v) => unsafe { std::env::set_var(name, v) },
            // TODO: Audit that the environment access only happens in single-threaded code.
            None => unsafe { std::env::remove_var(name) },
        }
        out
    }

    /// Run `body` with `name` guaranteed unset, restoring the prior value after.
    fn without_env<T>(name: &str, body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(name).ok();
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var(name) };
        let out = body();
        if let Some(v) = prev {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::set_var(name, v) };
        }
        out
    }

    /// Run `body` with all `names` guaranteed unset, restoring each afterwards.
    ///
    /// Acquires `ENV_LOCK` exactly once — `without_env` is NOT reentrant (the std
    /// `Mutex` would deadlock), so multi-var scrubbing must take the lock once.
    fn without_envs<T>(names: &[&str], body: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, Option<String>)> = names
            .iter()
            .map(|n| ((*n).to_owned(), std::env::var(n).ok()))
            .collect();
        for n in names {
            // TODO: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var(n) };
        }
        let out = body();
        for (n, v) in prev {
            match v {
                // TODO: Audit that the environment access only happens in single-threaded code.
                Some(v) => unsafe { std::env::set_var(&n, v) },
                // TODO: Audit that the environment access only happens in single-threaded code.
                None => unsafe { std::env::remove_var(&n) },
            }
        }
        out
    }

    // ── precedence engine: flag > env > toml > default ────────────────────

    #[test]
    fn flag_beats_everything() {
        // A unique env name keeps this deterministic regardless of ambient env.
        let (v, o) = pick_req(
            Some("flagv".to_owned()),
            "SPEAK_TEST_UNSET_A",
            Some("tomlv".to_owned()),
            "def".to_owned(),
        );
        assert_eq!(o, Origin::Flag);
        assert_eq!(v, "flagv");
    }

    #[test]
    fn env_beats_toml_and_default() {
        with_env("SPEAK_TEST_PICK_ENV", "envv", || {
            let (v, o) = pick_req(
                None::<String>,
                "SPEAK_TEST_PICK_ENV",
                Some("t".to_owned()),
                "d".to_owned(),
            );
            assert_eq!(o, Origin::Env);
            assert_eq!(v, "envv");
        });
    }

    #[test]
    fn toml_beats_default_when_no_flag_or_env() {
        without_env("SPEAK_TEST_UNSET_B", || {
            let (v, o) = pick_req(
                None,
                "SPEAK_TEST_UNSET_B",
                Some("tomlv".to_owned()),
                "def".to_owned(),
            );
            assert_eq!(o, Origin::Toml);
            assert_eq!(v, "tomlv");
        });
    }

    #[test]
    fn default_when_nothing_else_present() {
        without_env("SPEAK_TEST_UNSET_C", || {
            let (v, o) = pick_req(None, "SPEAK_TEST_UNSET_C", None, "def".to_owned());
            assert_eq!(o, Origin::Default);
            assert_eq!(v, "def");
        });
    }

    #[test]
    fn opt_default_origin_is_default_when_absent() {
        without_env("SPEAK_TEST_UNSET_D", || {
            let (v, o) = pick_opt::<String>(None, "SPEAK_TEST_UNSET_D", None);
            assert_eq!(v, None);
            assert_eq!(o, Origin::Default);
        });
    }

    #[test]
    fn opt_env_parses_typed_value() {
        with_env("SPEAK_TEST_PICK_OPT", "5", || {
            let (v, o) = pick_opt::<u32>(None, "SPEAK_TEST_PICK_OPT", Some(9));
            assert_eq!(v, Some(5));
            assert_eq!(o, Origin::Env);
        });
    }

    #[test]
    fn empty_env_is_ignored_and_falls_through_to_toml() {
        with_env("SPEAK_TEST_EMPTY", "", || {
            let (v, o) = pick_req(
                None,
                "SPEAK_TEST_EMPTY",
                Some("tomlv".to_owned()),
                "def".to_owned(),
            );
            assert_eq!(o, Origin::Toml);
            assert_eq!(v, "tomlv");
        });
    }

    // ── resolver pipeline records origin end-to-end ───────────────────────

    /// Find a recorded entry by key.
    fn entry<'a>(cfg: &'a Config, key: &str) -> &'a (String, String, Origin) {
        cfg.entries()
            .iter()
            .find(|(k, ..)| k == key)
            .unwrap_or_else(|| panic!("no entry {key}"))
    }

    #[test]
    fn resolver_records_toml_origin_for_host() {
        without_env("SPEAK_HOST", || {
            let file = FileConfig {
                server: FileServer {
                    host: Some("http://toml-host:1".into()),
                    ..FileServer::default()
                },
                ..FileConfig::default()
            };
            let cfg = Resolver::new(GlobalFlags::default(), file)
                .finish()
                .unwrap();
            let (_, value, origin) = entry(&cfg, "server.host");
            assert_eq!(value, "http://toml-host:1");
            assert_eq!(*origin, Origin::Toml);
        });
    }

    #[test]
    fn resolver_flag_overrides_toml_for_host() {
        without_env("SPEAK_HOST", || {
            let file = FileConfig {
                server: FileServer {
                    host: Some("http://toml-host:1".into()),
                    ..FileServer::default()
                },
                ..FileConfig::default()
            };
            let flags = GlobalFlags {
                host: Some("http://flag-host:2".into()),
                ..GlobalFlags::default()
            };
            let cfg = Resolver::new(flags, file).finish().unwrap();
            let (_, value, origin) = entry(&cfg, "server.host");
            assert_eq!(value, "http://flag-host:2");
            assert_eq!(*origin, Origin::Flag);
        });
    }

    #[test]
    fn resolver_default_host_when_unset() {
        without_env("SPEAK_HOST", || {
            let cfg = Resolver::new(GlobalFlags::default(), FileConfig::default())
                .finish()
                .unwrap();
            let (_, value, origin) = entry(&cfg, "server.host");
            assert_eq!(value, DEFAULT_HOST);
            assert_eq!(*origin, Origin::Default);
        });
    }

    #[test]
    fn resolver_env_overrides_toml_for_retry_max() {
        // FR: SPEAK_RETRY_MAX=5 surfaces in `config show` with origin env.
        with_env("SPEAK_RETRY_MAX", "5", || {
            let cfg = Resolver::new(GlobalFlags::default(), FileConfig::default())
                .finish()
                .unwrap();
            let (_, value, origin) = entry(&cfg, "retry.max_retries");
            assert_eq!(value, "5");
            assert_eq!(*origin, Origin::Env);
            assert_eq!(cfg.retry.policy.max_retries, 5);
        });
    }

    #[test]
    fn http_section_resolves_save_dir_with_toml_origin() {
        without_env("SPEAK_SAVE_DIR", || {
            let file = FileConfig {
                http: FileHttp {
                    save_dir: Some("/srv/audio".into()),
                    ..FileHttp::default()
                },
                ..FileConfig::default()
            };
            let cfg = Resolver::new(GlobalFlags::default(), file)
                .finish()
                .unwrap();
            let (_, value, origin) = entry(&cfg, "http.save_dir");
            assert_eq!(value, "/srv/audio");
            assert_eq!(*origin, Origin::Toml);
            assert_eq!(
                cfg.http.save_dir.as_deref(),
                Some(std::path::Path::new("/srv/audio"))
            );
        });
    }

    #[test]
    fn http_migrates_translate_keys_from_legacy_general_section() {
        // An unmigrated file with the keys still under `[general]` keeps working.
        without_envs(&["SPEAK_TRANSLATE_URL", "SPEAK_TRANSLATE_MODEL"], || {
            let file = FileConfig {
                general: FileGeneral {
                    translate_url: Some("http://legacy/chat".into()),
                    translate_model: Some("legacy-mt".into()),
                    ..FileGeneral::default()
                },
                ..FileConfig::default()
            };
            let cfg = Resolver::new(GlobalFlags::default(), file)
                .finish()
                .unwrap();
            let (_, url, url_origin) = entry(&cfg, "http.translate_url");
            assert_eq!(url, "http://legacy/chat");
            assert_eq!(*url_origin, Origin::Toml);
            assert_eq!(
                cfg.http.translate_url.as_deref(),
                Some("http://legacy/chat")
            );
            assert_eq!(cfg.http.translate_model.as_deref(), Some("legacy-mt"));
        });
    }

    #[test]
    fn http_section_wins_over_legacy_general_for_same_key() {
        without_env("SPEAK_TRANSLATE_URL", || {
            let file = FileConfig {
                general: FileGeneral {
                    translate_url: Some("http://legacy/chat".into()),
                    ..FileGeneral::default()
                },
                http: FileHttp {
                    translate_url: Some("http://new/chat".into()),
                    ..FileHttp::default()
                },
                ..FileConfig::default()
            };
            let cfg = Resolver::new(GlobalFlags::default(), file)
                .finish()
                .unwrap();
            assert_eq!(cfg.http.translate_url.as_deref(), Some("http://new/chat"));
        });
    }

    #[test]
    fn env_overrides_http_translate_url() {
        with_env("SPEAK_TRANSLATE_URL", "http://env/chat", || {
            let cfg = Resolver::new(GlobalFlags::default(), FileConfig::default())
                .finish()
                .unwrap();
            let (_, value, origin) = entry(&cfg, "http.translate_url");
            assert_eq!(value, "http://env/chat");
            assert_eq!(*origin, Origin::Env);
        });
    }

    #[test]
    fn api_key_is_masked_in_entries() {
        without_env("SPEAK_API_KEY", || {
            let file = FileConfig {
                server: FileServer {
                    api_key: Some("super-secret".into()),
                    ..FileServer::default()
                },
                ..FileConfig::default()
            };
            let cfg = Resolver::new(GlobalFlags::default(), file)
                .finish()
                .unwrap();
            let (_, value, origin) = entry(&cfg, "server.api_key");
            assert_eq!(value, "***");
            assert_eq!(*origin, Origin::Toml);
            assert_eq!(cfg.server.api_key.as_deref(), Some("super-secret"));
        });
    }

    #[test]
    fn origin_display_strings() {
        assert_eq!(Origin::Flag.to_string(), "flag");
        assert_eq!(Origin::Env.to_string(), "env");
        assert_eq!(Origin::Toml.to_string(), "toml");
        assert_eq!(Origin::Default.to_string(), "default");
    }

    #[test]
    fn default_user_agent_carries_crate_version() {
        let ua = default_user_agent();
        assert!(ua.starts_with("speak/"), "{ua}");
        assert!(ua.contains(env!("CARGO_PKG_VERSION")));
    }
}
