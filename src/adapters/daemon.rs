//! Single-binary persistent-connection daemon (ADR-0005), routed through the
//! shared application **Facade** (T053).
//!
//! `speak daemon` runs a long-lived process that holds ONE warm
//! [`SpeakFacade`] over the keep-alive [`OpenAiAdapter`] pool (wrapped in its
//! [`Retry`] decorator) and a [`HeadlessAudio`] role, listening on a Unix
//! socket. CLI invocations forward their NETWORK speech-port calls to it through
//! the [`DaemonSpeechAdapter`] (length-prefixed framing), so a request takes the
//! IDENTICAL use-case path whether it runs in-process or over the warm daemon —
//! the only difference is which `Speech` adapter the use case calls. Local audio
//! (playback, capture) always stays in the foreground CLI, so `record`/`realtime`
//! capture and `say` playback are never forwarded; `say` runs synthesize-only on
//! the daemon (`play = false`) and the CLI plays the returned bytes locally.
//!
//! Wire shape: every message is two length-prefixed frames — a JSON header
//! ([`Request`] / [`Reply`]) followed by a binary payload (audio in/out; empty
//! when unused). `daemon stop` / `daemon status` are control ops handled without
//! the Facade.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

use crate::adapters::config::Config;
use crate::adapters::headless::HeadlessAudio;
use crate::adapters::libav::{DecodeOptions, LibavCodec};
use crate::adapters::openai::OpenAiAdapter;
use crate::adapters::retry::{Retry, jitter_entropy};
use crate::application::{SayOptions, SpeakFacade};
use crate::domain::audio_format::AudioFormat;
use crate::domain::language::Language;
use crate::domain::retry::{ErrorKind, RetryPolicy};
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::{StandardVoice, Voice, VoiceClone, VoiceMode};
use crate::domain::voice_design::VoiceDesign;
use crate::ports::audio::AudioSink;
use crate::ports::codec::AudioDecoder;
use crate::ports::probe::ServerProbe;
use crate::ports::synthesizer::{SynthesizedAudio, Synthesizer};
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;
use crate::ports::voice::VoiceRepository;

/// The daemon's concrete warm Facade: the retry-wrapped `openai` adapter, the
/// headless audio role, and the `libav` codec.
type DaemonFacade = SpeakFacade<Retry<OpenAiAdapter>, HeadlessAudio, LibavCodec>;

/// `daemon` subcommands (absent => start the server).
#[derive(clap::Subcommand, Debug)]
pub enum DaemonCmd {
    /// Stop a running daemon.
    Stop,
    /// Print daemon status JSON.
    Status,
}

/// `daemon` arguments.
#[derive(clap::Args, Debug)]
pub struct DaemonArgs {
    /// Run attached in the foreground (also the current default).
    #[arg(long)]
    pub foreground: bool,
    #[command(subcommand)]
    action: Option<DaemonCmd>,
}

// --------------------------------------------------------------------------
// Wire protocol (Facade operations + their replies)
// --------------------------------------------------------------------------

/// A high-level speech-port operation forwarded to the daemon's shared Facade.
/// Binary audio rides a SEPARATE frame, never this JSON header.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    /// Synthesize a spec (the daemon runs `say` with `play = false`).
    Synthesize { spec: SpeechSpecDto },
    /// Transcribe the payload audio (filename + optional language + format).
    Transcribe {
        filename: String,
        language: Option<String>,
        format: String,
    },
    /// Translate the payload audio into the `target` language.
    Translate { filename: String, target: String },
    /// Register the payload audio as a saved voice.
    AddVoice {
        name: String,
        ref_text: Option<String>,
    },
    /// List the saved voices.
    ListVoices,
    /// Delete a saved voice by name.
    RemoveVoice { name: String },
    /// Probe server health + advertised models + realtime capability.
    Health,
    /// Control: stop the daemon.
    Stop,
    /// Control: report daemon status.
    Status,
}

/// The daemon's reply header; a binary payload (Synthesize audio) rides a
/// second frame.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "result", rename_all = "snake_case")]
enum Reply {
    /// A side-effect op succeeded (add/remove voice, stop).
    Ok,
    /// The op failed with this message.
    Error { message: String },
    /// Synthesized audio metadata; the bytes ride the payload frame.
    Audio {
        content_type: String,
        rtf: Option<String>,
        audio_seconds: Option<String>,
    },
    /// A transcript / translation result.
    Text { text: String },
    /// The saved-voice listing.
    Voices { voices: Vec<VoiceDto> },
    /// The server health snapshot.
    Health {
        healthy: bool,
        models: Vec<String>,
        realtime: bool,
    },
    /// The control status document.
    Status { status: Value },
}

/// Wire projection of [`SpeechSpec`] (domain stays serde-free, ADR-0003).
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct SpeechSpecDto {
    input: String,
    voice: VoiceModeDto,
    format: String,
    language: String,
    speed: f32,
    gen_params: Map<String, Value>,
}

/// Wire projection of the [`VoiceMode`] Strategy.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum VoiceModeDto {
    Design {
        instruct: String,
    },
    Clone {
        name: String,
        ref_text: Option<String>,
    },
    Standard {
        name: String,
    },
}

/// Wire projection of a saved [`Voice`].
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct VoiceDto {
    name: String,
    has_ref_text: bool,
}

impl SpeechSpecDto {
    /// Project a domain spec onto its wire DTO.
    fn from_spec(spec: &SpeechSpec) -> Self {
        Self {
            input: spec.input().to_owned(),
            voice: VoiceModeDto::from_mode(spec.voice()),
            format: spec.format().as_str().to_owned(),
            language: spec.language().as_str().to_owned(),
            speed: spec.speed(),
            gen_params: spec.gen_params().clone(),
        }
    }

    /// Rebuild the validated domain spec from the wire DTO.
    fn into_spec(self) -> Result<SpeechSpec> {
        Ok(SpeechSpec::builder(&self.input)
            .voice(self.voice.into_mode()?)
            .language(Language::parse(&self.language)?)
            .format(AudioFormat::parse(&self.format)?)
            .speed(self.speed)
            .gen_params(self.gen_params)
            .build()?)
    }
}

impl VoiceModeDto {
    /// Project a domain voice mode onto its wire DTO.
    fn from_mode(mode: &VoiceMode) -> Self {
        match mode {
            VoiceMode::Design(design) => Self::Design {
                instruct: design.instruct(),
            },
            VoiceMode::Clone(clone) => Self::Clone {
                name: clone.name().to_owned(),
                ref_text: clone.ref_text().map(ToOwned::to_owned),
            },
            VoiceMode::Standard(voice) => Self::Standard {
                name: voice.name().to_owned(),
            },
        }
    }

    /// Rebuild the validated domain voice mode from the wire DTO.
    fn into_mode(self) -> Result<VoiceMode> {
        Ok(match self {
            Self::Design { instruct } => VoiceMode::Design(VoiceDesign::parse(&instruct)?),
            Self::Clone { name, ref_text } => {
                VoiceMode::Clone(VoiceClone::new(&name, ref_text.as_deref())?)
            }
            Self::Standard { name } => VoiceMode::Standard(StandardVoice::new(&name)?),
        })
    }
}

impl VoiceDto {
    /// Project a domain voice onto its wire DTO.
    fn from_voice(voice: &Voice) -> Self {
        Self {
            name: voice.name().to_owned(),
            has_ref_text: voice.has_ref_text(),
        }
    }

    /// Rebuild the domain voice from the wire DTO.
    fn into_voice(self) -> Result<Voice> {
        Ok(Voice::new(&self.name, self.has_ref_text)?)
    }
}

/// Dispatch `daemon` subcommands.
pub async fn run(cfg: &Config, args: DaemonArgs) -> Result<()> {
    match args.action {
        None => start(cfg, args.foreground).await,
        Some(DaemonCmd::Stop) => stop(cfg).await,
        Some(DaemonCmd::Status) => status(cfg).await,
    }
}

/// True when a daemon is accepting connections on `socket`.
pub async fn is_running(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

// --------------------------------------------------------------------------
// Driven adapter: forward the speech ports to a running daemon (T053)
// --------------------------------------------------------------------------

/// A `Speech` adapter that forwards every network port call to a running daemon
/// over its Unix socket, so the daemon's warm pool services the request.
///
/// It implements the SAME five ports the in-process `openai` adapter does, so the
/// composition root can inject it in place of `Retry<OpenAiAdapter>` without the
/// use cases ever knowing. Transient socket failures are retried under the
/// injected [`RetryPolicy`] (the CommandTransport decorator, T046 / ADR-0005).
pub struct DaemonSpeechAdapter {
    socket: PathBuf,
    policy: RetryPolicy,
    jitter_seed: Option<u64>,
}

impl DaemonSpeechAdapter {
    /// Bind the forwarding adapter to `socket`, retrying under `policy`.
    #[must_use]
    pub fn new(socket: PathBuf, policy: RetryPolicy, jitter_seed: Option<u64>) -> Self {
        Self {
            socket,
            policy,
            jitter_seed,
        }
    }

    /// Forward `request` (+ optional binary `payload`) to the daemon, retrying a
    /// transient socket connect/IO failure under the bounded policy.
    async fn call(&self, request: &Request, payload: &[u8]) -> Result<(Reply, Vec<u8>)> {
        let mut attempt = 0u32;
        loop {
            match forward(&self.socket, request, payload).await {
                Ok(out) => return Ok(out),
                Err(err) if self.policy.should_retry(attempt, ErrorKind::Connect) => {
                    let delay = self
                        .policy
                        .delay_for(attempt, jitter_entropy(self.jitter_seed, attempt));
                    tracing::debug!(attempt, "retrying daemon socket call: {err:#}");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Forward an op that expects a bare `Ok` acknowledgement.
    async fn call_ok(&self, request: &Request, payload: &[u8]) -> Result<()> {
        match self.call(request, payload).await?.0 {
            Reply::Ok => Ok(()),
            other => Err(unexpected(&other)),
        }
    }

    /// Forward the `Health` op once, returning the snapshot tuple.
    async fn fetch_health(&self) -> Result<(bool, Vec<String>, bool)> {
        match self.call(&Request::Health, &[]).await?.0 {
            Reply::Health {
                healthy,
                models,
                realtime,
            } => Ok((healthy, models, realtime)),
            other => Err(unexpected(&other)),
        }
    }
}

impl Synthesizer for DaemonSpeechAdapter {
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        let request = Request::Synthesize {
            spec: SpeechSpecDto::from_spec(spec),
        };
        let (reply, bytes) = self.call(&request, &[]).await?;
        match reply {
            Reply::Audio {
                content_type,
                rtf,
                audio_seconds,
            } => Ok(SynthesizedAudio {
                bytes,
                content_type,
                rtf,
                audio_seconds,
            }),
            other => Err(unexpected(&other)),
        }
    }
}

impl Transcriber for DaemonSpeechAdapter {
    async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String> {
        let request = Request::Transcribe {
            filename: req.filename.to_owned(),
            language: req.language.map(|l| l.as_str().to_owned()),
            format: req.format.to_owned(),
        };
        text_reply(self.call(&request, req.audio).await?.0)
    }
}

impl Translator for DaemonSpeechAdapter {
    async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String> {
        let request = Request::Translate {
            filename: filename.to_owned(),
            target: target.as_str().to_owned(),
        };
        text_reply(self.call(&request, audio).await?.0)
    }
}

impl VoiceRepository for DaemonSpeechAdapter {
    async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        let request = Request::AddVoice {
            name: name.to_owned(),
            ref_text: ref_text.map(ToOwned::to_owned),
        };
        self.call_ok(&request, audio).await
    }

    async fn list(&self) -> Result<Vec<Voice>> {
        match self.call(&Request::ListVoices, &[]).await?.0 {
            Reply::Voices { voices } => voices.into_iter().map(VoiceDto::into_voice).collect(),
            other => Err(unexpected(&other)),
        }
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.call_ok(
            &Request::RemoveVoice {
                name: name.to_owned(),
            },
            &[],
        )
        .await
    }
}

impl ServerProbe for DaemonSpeechAdapter {
    async fn health(&self) -> Result<bool> {
        Ok(self.fetch_health().await?.0)
    }

    async fn models(&self) -> Result<Vec<String>> {
        Ok(self.fetch_health().await?.1)
    }

    async fn supports_realtime(&self) -> Result<bool> {
        Ok(self.fetch_health().await?.2)
    }
}

/// Interpret a reply expected to carry transcript/translation text.
fn text_reply(reply: Reply) -> Result<String> {
    match reply {
        Reply::Text { text } => Ok(text),
        other => Err(unexpected(&other)),
    }
}

/// Surface a daemon-side error reply or an unexpected variant as an error.
fn unexpected(reply: &Reply) -> anyhow::Error {
    match reply {
        Reply::Error { message } => anyhow::anyhow!("daemon error: {message}"),
        other => anyhow::anyhow!("daemon returned an unexpected reply: {other:?}"),
    }
}

/// Connect to `socket`, send the request + payload frames, read the reply frames.
async fn forward(socket: &Path, request: &Request, payload: &[u8]) -> Result<(Reply, Vec<u8>)> {
    let mut stream = UnixStream::connect(socket)
        .await
        .context("connecting to daemon socket")?;
    write_frame(&mut stream, &serde_json::to_vec(request)?).await?;
    write_frame(&mut stream, payload).await?;
    let reply: Reply = serde_json::from_slice(&read_frame(&mut stream).await?)?;
    let body = read_frame(&mut stream).await?;
    Ok((reply, body))
}

// --------------------------------------------------------------------------
// Server
// --------------------------------------------------------------------------

struct State {
    facade: DaemonFacade,
    started: Instant,
    requests: AtomicU64,
    socket: PathBuf,
    host: String,
    idle_timeout: u64,
    shutdown: Notify,
    last_active: std::sync::Mutex<Instant>,
}

/// Build the daemon's warm Facade (retry-wrapped openai + headless audio + libav).
fn build_facade(cfg: &Config) -> Result<DaemonFacade> {
    let speech = OpenAiAdapter::new(cfg)?;
    let speech = Retry::new(speech, cfg.retry.policy, cfg.retry.jitter_seed);
    let codec = LibavCodec::new(DecodeOptions {
        threads: cfg.ffmpeg.threads,
        log_level: cfg.ffmpeg.log_level.clone(),
    });
    Ok(SpeakFacade::new(speech, HeadlessAudio::new(), codec))
}

async fn start(cfg: &Config, _foreground: bool) -> Result<()> {
    let socket = cfg.daemon.socket.clone();
    if is_running(&socket).await {
        bail!("daemon already running at {}", socket.display());
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(&socket);
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    let state = Arc::new(State {
        facade: build_facade(cfg)?,
        started: Instant::now(),
        requests: AtomicU64::new(0),
        socket: socket.clone(),
        host: cfg.server.host.clone(),
        idle_timeout: cfg.daemon.idle_timeout,
        shutdown: Notify::new(),
        last_active: std::sync::Mutex::new(Instant::now()),
    });
    tracing::info!(socket = %socket.display(), host = %state.host, "daemon listening");
    if !cfg.general.quiet {
        eprintln!(
            "speak daemon listening at {} (host {})",
            socket.display(),
            state.host
        );
    }
    accept_loop(&listener, &state).await;
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

async fn accept_loop(listener: &UnixListener, state: &Arc<State>) {
    spawn_idle_watch(state);
    loop {
        tokio::select! {
            biased;
            () = state.shutdown.notified() => break,
            _ = tokio::signal::ctrl_c() => break,
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    let state = Arc::clone(state);
                    tokio::spawn(async move {
                        if let Err(e) = serve(stream, &state).await {
                            tracing::warn!("daemon connection error: {e:#}");
                        }
                    });
                }
            }
        }
    }
}

fn spawn_idle_watch(state: &Arc<State>) {
    if state.idle_timeout == 0 {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let idle = state
                .last_active
                .lock()
                .map(|t| t.elapsed().as_secs())
                .unwrap_or(0);
            if idle >= state.idle_timeout {
                state.shutdown.notify_one();
                break;
            }
        }
    });
}

async fn serve(mut stream: UnixStream, state: &Arc<State>) -> Result<()> {
    let request: Request = serde_json::from_slice(&read_frame(&mut stream).await?)?;
    let payload = read_frame(&mut stream).await?;
    if let Ok(mut t) = state.last_active.lock() {
        *t = Instant::now();
    }
    match request {
        Request::Stop => {
            write_reply(&mut stream, &Reply::Ok, &[]).await?;
            state.shutdown.notify_one();
        }
        Request::Status => {
            let status = status_body(state);
            write_reply(&mut stream, &Reply::Status { status }, &[]).await?;
        }
        other => {
            state.requests.fetch_add(1, Ordering::Relaxed);
            let (reply, body) = dispatch(other, payload, &state.facade)
                .await
                .unwrap_or_else(|e| {
                    (
                        Reply::Error {
                            message: format!("{e:#}"),
                        },
                        Vec::new(),
                    )
                });
            write_reply(&mut stream, &reply, &body).await?;
        }
    }
    Ok(())
}

/// Route one framed operation through the shared application Facade — the SAME
/// use cases the CLI runs (T053). Generic over the Facade's roles so it is
/// exercised over the in-memory port doubles in tests.
async fn dispatch<S, A, K>(
    request: Request,
    payload: Vec<u8>,
    facade: &SpeakFacade<S, A, K>,
) -> Result<(Reply, Vec<u8>)>
where
    S: Synthesizer + Transcriber + Translator + VoiceRepository + ServerProbe,
    A: AudioSink,
    K: AudioDecoder,
{
    match request {
        Request::Synthesize { spec } => {
            let spec = spec.into_spec()?;
            let opts = SayOptions {
                play: false,
                volume: 1.0,
                devices: Vec::new(),
            };
            let audio = facade.say(&spec, &opts).await?.audio;
            let reply = Reply::Audio {
                content_type: audio.content_type,
                rtf: audio.rtf,
                audio_seconds: audio.audio_seconds,
            };
            Ok((reply, audio.bytes))
        }
        Request::Transcribe {
            filename,
            language,
            format,
        } => {
            let language = language.map(|l| Language::parse(&l)).transpose()?;
            let req = TranscribeRequest {
                audio: &payload,
                filename: &filename,
                language: language.as_ref(),
                format: &format,
            };
            Ok((
                Reply::Text {
                    text: facade.transcribe(&req).await?,
                },
                Vec::new(),
            ))
        }
        Request::Translate { filename, target } => {
            let target = Language::parse(&target)?;
            let text = facade.translate(&payload, &filename, &target).await?;
            Ok((Reply::Text { text }, Vec::new()))
        }
        Request::AddVoice { name, ref_text } => {
            facade
                .add_voice(&name, &payload, ref_text.as_deref())
                .await?;
            Ok((Reply::Ok, Vec::new()))
        }
        Request::ListVoices => {
            let voices = facade
                .list_voices()
                .await?
                .iter()
                .map(VoiceDto::from_voice)
                .collect();
            Ok((Reply::Voices { voices }, Vec::new()))
        }
        Request::RemoveVoice { name } => {
            facade.remove_voice(&name).await?;
            Ok((Reply::Ok, Vec::new()))
        }
        Request::Health => {
            let h = facade.health().await?;
            Ok((
                Reply::Health {
                    healthy: h.healthy,
                    models: h.models,
                    realtime: h.realtime,
                },
                Vec::new(),
            ))
        }
        Request::Stop | Request::Status => {
            bail!("control op routed to the Facade dispatcher")
        }
    }
}

fn status_body(state: &State) -> Value {
    json!({
        "pid": std::process::id(),
        "uptime_secs": state.started.elapsed().as_secs(),
        "requests": state.requests.load(Ordering::Relaxed),
        "socket": state.socket.display().to_string(),
        "host": state.host,
    })
}

async fn write_reply(stream: &mut UnixStream, reply: &Reply, body: &[u8]) -> Result<()> {
    write_frame(stream, &serde_json::to_vec(reply)?).await?;
    write_frame(stream, body).await
}

async fn stop(cfg: &Config) -> Result<()> {
    let socket = &cfg.daemon.socket;
    if !is_running(socket).await {
        println!("no daemon running at {}", socket.display());
        return Ok(());
    }
    forward(socket, &Request::Stop, &[]).await?;
    println!("stopped daemon at {}", socket.display());
    Ok(())
}

async fn status(cfg: &Config) -> Result<()> {
    let socket = &cfg.daemon.socket;
    if !is_running(socket).await {
        println!(
            "{}",
            json!({"running": false, "socket": socket.display().to_string()})
        );
        return Ok(());
    }
    let mut value = match forward(socket, &Request::Status, &[]).await?.0 {
        Reply::Status { status } => status,
        Reply::Error { message } => bail!("daemon error: {message}"),
        _ => json!({}),
    };
    if let Some(map) = value.as_object_mut() {
        map.insert("running".into(), json!(true));
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn write_frame(stream: &mut UnixStream, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len()).context("frame too large")?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let len = stream.read_u32().await?;
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec, FakeSpeech};

    fn fake_facade() -> SpeakFacade<FakeSpeech, FakeAudio, FakeCodec> {
        SpeakFacade::new(FakeSpeech::default(), FakeAudio::default(), FakeCodec)
    }

    fn sample_spec(mode: VoiceMode) -> SpeechSpec {
        SpeechSpec::builder("hi there")
            .voice(mode)
            .language(Language::parse("en").unwrap())
            .format(AudioFormat::Flac)
            .speed(1.5)
            .build()
            .unwrap()
    }

    #[test]
    fn speech_spec_dto_round_trips_each_voice_mode() {
        let modes = [
            VoiceMode::Design(VoiceDesign::parse("whisper, british accent").unwrap()),
            VoiceMode::Clone(VoiceClone::new("narrator", Some("the quick fox")).unwrap()),
            VoiceMode::Standard(StandardVoice::new("alloy").unwrap()),
        ];
        for mode in modes {
            let spec = sample_spec(mode);
            let back = SpeechSpecDto::from_spec(&spec).into_spec().unwrap();
            assert_eq!(back, spec);
        }
    }

    #[test]
    fn request_serde_round_trips() {
        let req = Request::Transcribe {
            filename: "a.wav".into(),
            language: Some("pt".into()),
            format: "json".into(),
        };
        let bytes = serde_json::to_vec(&req).unwrap();
        assert_eq!(serde_json::from_slice::<Request>(&bytes).unwrap(), req);
    }

    #[tokio::test]
    async fn dispatch_synthesize_routes_through_the_facade() {
        let facade = fake_facade();
        let spec = SpeechSpecDto::from_spec(&sample_spec(VoiceMode::Standard(
            StandardVoice::new("alloy").unwrap(),
        )));
        let (reply, body) = dispatch(Request::Synthesize { spec }, Vec::new(), &facade)
            .await
            .unwrap();
        assert!(matches!(reply, Reply::Audio { .. }));
        assert_eq!(body, b"AUDIO");
    }

    #[tokio::test]
    async fn dispatch_transcribe_and_translate_return_text() {
        let facade = fake_facade();
        let (t, _) = dispatch(
            Request::Transcribe {
                filename: "a.wav".into(),
                language: None,
                format: "json".into(),
            },
            b"\x00".to_vec(),
            &facade,
        )
        .await
        .unwrap();
        assert_eq!(
            t,
            Reply::Text {
                text: "hello".into()
            }
        );
        let (tr, _) = dispatch(
            Request::Translate {
                filename: "a.wav".into(),
                target: "en".into(),
            },
            b"\x00".to_vec(),
            &facade,
        )
        .await
        .unwrap();
        assert_eq!(
            tr,
            Reply::Text {
                text: "olá".into()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_voices_round_trips_through_the_facade() {
        let facade = fake_facade();
        dispatch(
            Request::AddVoice {
                name: "narrator".into(),
                ref_text: Some("ref".into()),
            },
            b"\x00".to_vec(),
            &facade,
        )
        .await
        .unwrap();
        let (listing, _) = dispatch(Request::ListVoices, Vec::new(), &facade)
            .await
            .unwrap();
        match listing {
            Reply::Voices { voices } => {
                assert_eq!(voices.len(), 1);
                assert_eq!(voices[0].name, "narrator");
                assert!(voices[0].has_ref_text);
            }
            other => panic!("expected Voices, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_health_reports_the_snapshot() {
        let (reply, _) = dispatch(Request::Health, Vec::new(), &fake_facade())
            .await
            .unwrap();
        assert_eq!(
            reply,
            Reply::Health {
                healthy: true,
                models: vec!["tts-1".into(), "whisper-1".into()],
                realtime: true,
            }
        );
    }

    #[tokio::test]
    async fn forward_round_trips_a_request_over_a_real_socket() {
        // Bind a one-shot server backed by the fake Facade, then forward a
        // Transcribe through the wire helpers and assert the routed result.
        let dir = std::env::temp_dir().join(format!("speak-daemon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("speak.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request: Request =
                serde_json::from_slice(&read_frame(&mut stream).await.unwrap()).unwrap();
            let payload = read_frame(&mut stream).await.unwrap();
            let (reply, body) = dispatch(request, payload, &fake_facade()).await.unwrap();
            write_reply(&mut stream, &reply, &body).await.unwrap();
        });

        let request = Request::Transcribe {
            filename: "clip.wav".into(),
            language: None,
            format: "json".into(),
        };
        let (reply, _) = forward(&socket, &request, b"audio-bytes").await.unwrap();
        server.await.unwrap();
        assert_eq!(
            reply,
            Reply::Text {
                text: "hello".into()
            }
        );
        let _ = std::fs::remove_file(&socket);
    }

    #[tokio::test]
    async fn frame_round_trips_arbitrary_bytes() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let payload = b"length-prefixed frame".to_vec();
        let expected = payload.clone();
        let writer = tokio::spawn(async move {
            write_frame(&mut a, &payload).await.unwrap();
        });
        let got = read_frame(&mut b).await.unwrap();
        writer.await.unwrap();
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn empty_frame_round_trips() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let writer = tokio::spawn(async move {
            write_frame(&mut a, &[]).await.unwrap();
        });
        assert!(read_frame(&mut b).await.unwrap().is_empty());
        writer.await.unwrap();
    }

    #[test]
    fn reply_serde_round_trips_audio_metadata() {
        let reply = Reply::Audio {
            content_type: "audio/mpeg".into(),
            rtf: Some("0.1".into()),
            audio_seconds: Some("2.0".into()),
        };
        let bytes = serde_json::to_vec(&reply).unwrap();
        assert_eq!(serde_json::from_slice::<Reply>(&bytes).unwrap(), reply);
    }
}
