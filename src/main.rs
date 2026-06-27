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

use speak::adapters::coreaudio::CoreAudio;
use speak::adapters::libav::{DecodeOptions, LibavCodec};
use speak::adapters::openai::OpenAiAdapter;
use speak::application::SpeakFacade;
use speak::config::Config;
use speak::{daemon, logging};

use cli::AppFacade;
use cli::args::{Cli, Command, GlobalArgs};

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

    /// Assemble the application Facade over the three concrete adapter roles.
    ///
    /// `native` routes `say` through the server's `/tts` endpoint; every other
    /// command leaves it at the configured `[tts].native` default.
    fn facade(&self, native: bool) -> Result<AppFacade> {
        let speech = OpenAiAdapter::new(self.cfg)?.with_native(native || self.cfg.tts.native);
        let codec = LibavCodec::new(DecodeOptions {
            threads: self.cfg.ffmpeg.threads,
            log_level: self.cfg.ffmpeg.log_level.clone(),
        });
        Ok(SpeakFacade::new(speech, CoreAudio::new(), codec))
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
        Command::Health => cli::check::health(cfg, out).await,
        Command::Devices(args) => cli::devices::run(&args),
        Command::Config { action } => cli::config::run(action, cfg, out),
        Command::Say(args) => {
            cli::say::run(&factory.facade(args.native)?, cfg, &globals, args).await
        }
        Command::Transcribe(args) => cli::transcribe::run(&factory.facade(false)?, cfg, args).await,
        Command::Translate(args) => cli::translate::run(&factory.facade(false)?, cfg, args).await,
        Command::Realtime(args) => cli::realtime::run(cfg, &globals, args).await,
        Command::Voices { action } => cli::voices::run(&factory.facade(false)?, action, out).await,
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
