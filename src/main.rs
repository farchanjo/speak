//! `speak` — a network client for an OpenAI-compatible speech server.
//!
//! This binary is the **composition root** (ADR-0003): it parses the CLI, loads
//! the layered configuration, builds the concrete adapter object graph (the
//! `openai` speech adapter, the `coreaudio` audio adapter, and the `libav` codec
//! adapter wired into the application [`SpeakFacade`]), and dispatches each
//! subcommand to its driving-adapter handler in [`cli`]. It holds no business
//! logic of its own — that lives in `speak::application`, behind the ports.
//!
//! Media path: server audio is decoded and resampled with linked `libav*`
//! (ffmpeg-the-third) and played through the native macOS CoreAudio mixer
//! (AVAudioEngine); the microphone is captured natively too. Nothing is shelled
//! out.

mod cli;

use anyhow::Result;
use clap::Parser;

use speak::adapters::chatmt::ChatMtTranslator;
use speak::adapters::config::Config;
use speak::adapters::coreaudio::CoreAudio;
use speak::adapters::libav::{DecodeOptions, LibavCodec};
use speak::adapters::openai::OpenAiAdapter;
use speak::adapters::retry::Retry;
use speak::adapters::sse::SseRealtimeClient;
use speak::application::SpeakFacade;
use speak::daemon::DaemonSpeechAdapter;
use speak::{daemon, logging};

use cli::AppFacade;
use cli::args::{Cli, Command, GlobalArgs};
use cli::speech::{DirectSpeech, SpeechRole};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // Parse first so `-v`/`--verbose` can size the console diagnostics layer;
    // RESULTS go to stdout via the Presenter, diagnostics to stderr + the file.
    let _log_guard = logging::init(cli.globals.verbose);
    run(cli).await
}

/// Resolve configuration and dispatch the parsed command.
async fn run(cli: Cli) -> Result<()> {
    if let Command::Completions { shell } = cli.command {
        return cli::completions::emit(shell);
    }
    let cfg = Config::load(cli.globals.flags())?;
    tracing::info!(
        host = %cfg.server.host,
        command = ?std::mem::discriminant(&cli.command),
        "dispatch"
    );
    dispatch(cli, &cfg).await
}

/// Composition-root **Factory** (T054): builds the concrete adapter object graph
/// and wires it into the application [`SpeakFacade`].
struct Factory<'a> {
    cfg: &'a Config,
}

impl<'a> Factory<'a> {
    /// Bind the Factory to the resolved configuration.
    fn new(cfg: &'a Config) -> Self {
        Self { cfg }
    }

    /// Assemble the application Facade over the three concrete adapter roles: the
    /// [`SpeechRole`] selector, the `coreaudio` I/O adapter, and the `libav`
    /// codec adapter. Local audio always stays in the foreground CLI.
    ///
    /// When a daemon is live the speech role forwards to it ([`DaemonSpeechAdapter`],
    /// T053); otherwise it is the in-process `openai` adapter wrapped in its
    /// port-preserving [`Retry`] decorator (T046), resolved from `[retry]`. Both
    /// implement the SAME ports, so the use cases are oblivious. `native` routes
    /// the in-process `say` through `/tts` (a forwarded `say` follows the daemon's
    /// `[tts].native` default).
    async fn facade(&self, native: bool) -> Result<AppFacade> {
        let speech = self.speech_role(native).await?;
        let codec = LibavCodec::new(DecodeOptions {
            threads: self.cfg.ffmpeg.threads,
            log_level: self.cfg.ffmpeg.log_level.clone(),
        });
        Ok(SpeakFacade::new(speech, CoreAudio::new(), codec))
    }

    /// Pick the speech role: forward to a running daemon, else go in-process.
    async fn speech_role(&self, native: bool) -> Result<SpeechRole> {
        let socket = &self.cfg.daemon.socket;
        if daemon::is_running(socket).await {
            tracing::debug!(socket = %socket.display(), "speech role: daemon-forward");
            return Ok(SpeechRole::Daemon(DaemonSpeechAdapter::new(
                socket.clone(),
                self.cfg.retry.policy,
                self.cfg.retry.jitter_seed,
            )));
        }
        tracing::debug!("speech role: in-process");
        let openai = OpenAiAdapter::new(self.cfg)?.with_native(native || self.cfg.tts.native);
        let speech = Retry::new(openai, self.cfg.retry.policy, self.cfg.retry.jitter_seed);
        let chatmt = self.chat_mt()?;
        Ok(SpeechRole::Direct(Box::new(DirectSpeech {
            speech,
            chatmt,
        })))
    }

    /// Build the chat-MT translate Strategy when `[http].translate_url` is set
    /// (T039); its own `openai` adapter transcribes the chunk before chat-MT.
    fn chat_mt(&self) -> Result<Option<ChatMtTranslator<OpenAiAdapter>>> {
        if self.cfg.http.translate_url.is_none() {
            return Ok(None);
        }
        ChatMtTranslator::new(OpenAiAdapter::new(self.cfg)?, self.cfg)
    }
}

/// Route the parsed command to its driving-adapter handler.
async fn dispatch(cli: Cli, cfg: &Config) -> Result<()> {
    let factory = Factory::new(cfg);
    let globals = cli.globals;
    let mut presenter = build_presenter(&globals, cfg, &cli.command);
    let out = presenter.as_mut();
    match cli.command {
        Command::Check => cli::check::check(cfg, out),
        Command::Health => cli::check::health(&factory.facade(false).await?, out).await,
        Command::Devices(_) => cli::devices::run(out),
        Command::Config { action } => cli::config::run(action, cfg, out),
        Command::Say(args) => {
            cli::say::run(
                &factory.facade(args.native).await?,
                cfg,
                &globals,
                args,
                out,
            )
            .await
        }
        Command::Transcribe(args) => {
            cli::transcribe::run(&factory.facade(false).await?, cfg, args, out).await
        }
        Command::Translate(args) => {
            cli::translate::run(&factory.facade(false).await?, cfg, args, out).await
        }
        Command::Realtime(args) => {
            let sse = SseRealtimeClient::new(cfg)?;
            cli::realtime::run(
                &factory.facade(false).await?,
                cfg,
                &globals,
                args,
                &sse,
                out,
            )
            .await
        }
        Command::Record(args) => {
            cli::record::run(&factory.facade(false).await?, cfg, args, out).await
        }
        Command::Voices { action } => {
            cli::voices::run(&factory.facade(false).await?, action, out).await
        }
        Command::Daemon(args) => daemon::run(cfg, args).await,
        Command::Completions { .. } => Ok(()),
    }
}

/// Select the output Presenter Strategy from the global flags + config (ADR-0009).
///
/// `--json` (or the per-command `devices --json`, or `[general].json`) picks the
/// machine-readable renderer; otherwise the console renderer honours `--quiet`
/// and the resolved `--color`/`NO_COLOR` behaviour.
fn build_presenter(
    globals: &GlobalArgs,
    cfg: &Config,
    command: &Command,
) -> Box<dyn speak::ports::presenter::Presenter> {
    let want_json =
        globals.json || cfg.general.json || matches!(command, Command::Devices(args) if args.json);
    let color = speak::adapters::presenter::color_enabled(cfg.general.color);
    speak::adapters::presenter::build(want_json, cfg.general.quiet, color)
}
