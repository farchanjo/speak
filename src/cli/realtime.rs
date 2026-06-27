//! `realtime` handler (T051): capture the microphone and re-voice it live (FR-8).
//!
//! A thin driving adapter: it maps the CLI flags to a [`RealtimeOptions`] value
//! object, then runs the application [`RealtimeUseCase`] (via the [`AppFacade`])
//! one chunk per loop iteration until Ctrl-C. The use case owns the
//! capture -> ASR -> (MT) -> re-voice pipeline and the playback routing; this
//! handler only resolves options, surfaces each caption through the [`Presenter`],
//! and sends loop diagnostics to `tracing`. The pipeline mode is the exclusive
//! `--translate` (default) / `--no-translate` / `--echo` group; the spoken output
//! voice is `--instruct` (design) or the global `--voice` (clone / standard).

use anyhow::Result;

use speak::application::{RealtimeOptions, RealtimeStep};
use speak::config::Config;
use speak::domain::audio_format::AudioFormat;
use speak::domain::language::Language;
use speak::domain::realtime::RealtimeMode;
use speak::ports::audio::AudioDeviceId;
use speak::ports::presenter::Presenter;

use super::AppFacade;
use super::args::{GlobalArgs, RealtimeArgs};
use super::say::gen_to_map;

/// Run the `realtime` subcommand loop until Ctrl-C.
pub async fn run(
    facade: &AppFacade,
    cfg: &Config,
    globals: &GlobalArgs,
    args: RealtimeArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let opts = build_options(cfg, globals, &args)?;
    tracing::info!(
        mode = opts.mode.as_str(),
        chunk = opts.chunk_secs,
        device = args.device,
        from = opts.from.as_ref().map_or("auto", Language::as_str),
        to = opts.to.as_str(),
        "realtime loop starting; Ctrl-C to stop"
    );
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("realtime loop stopping");
                return Ok(());
            }
            res = facade.realtime_step(&opts) => match res {
                Ok(Some(step)) => emit_step(&step, presenter)?,
                Ok(None) => {}
                Err(e) => tracing::warn!("realtime chunk failed: {e:#}"),
            },
        }
    }
}

/// Surface one processed chunk's spoken text through the Presenter.
fn emit_step(step: &RealtimeStep, presenter: &mut dyn Presenter) -> Result<()> {
    presenter.line(&step.output_text)
}

/// Assemble the [`RealtimeOptions`] value object from the flags + config.
fn build_options(
    cfg: &Config,
    globals: &GlobalArgs,
    args: &RealtimeArgs,
) -> Result<RealtimeOptions> {
    let mode = args.mode();
    let from = args
        .from
        .as_deref()
        .or(cfg.realtime.from.as_deref())
        .map(Language::parse)
        .transpose()?;
    let to = Language::parse(&args.to)?;
    let instruct = args.instruct.as_deref().or(cfg.tts.instruct.as_deref());
    let voice = super::resolve_voice(&cfg.tts.voice, globals.voice.is_some(), instruct, None)?;
    let default_lang = Language::parse(&cfg.tts.language)?;
    Ok(RealtimeOptions {
        output_language: output_language(mode, &to, from.as_ref(), &default_lang),
        mode,
        from,
        to,
        voice,
        format: AudioFormat::parse(&cfg.tts.format)?,
        speed: cfg.tts.speed,
        gen_params: gen_to_map(&cfg.tts.gen_params),
        chunk_secs: chunk_secs(cfg, args),
        device: (args.device != 0).then_some(AudioDeviceId(args.device)),
        outputs: args
            .output_device
            .iter()
            .copied()
            .map(AudioDeviceId)
            .collect(),
        volume: cfg.audio.output.volume,
        vad: cfg.audio.input.vad,
        silence_floor: silence_floor(cfg.audio.input.silence_threshold_db),
    })
}

/// The language the re-voiced output is spoken in: the target when translating,
/// otherwise the source hint (falling back to the configured default).
fn output_language(
    mode: RealtimeMode,
    to: &Language,
    from: Option<&Language>,
    default: &Language,
) -> Language {
    match mode {
        RealtimeMode::Translate => to.clone(),
        RealtimeMode::NoTranslate | RealtimeMode::Echo => {
            from.cloned().unwrap_or_else(|| default.clone())
        }
    }
}

/// Resolve the capture chunk length: the `--chunk` flag, or the configured
/// `[audio.input].chunk_secs` when the flag is left at its default.
fn chunk_secs(cfg: &Config, args: &RealtimeArgs) -> f64 {
    if args.chunk == 5 {
        cfg.audio.input.chunk_secs
    } else {
        f64::from(args.chunk as u32)
    }
}

/// Linear RMS floor from a silence threshold expressed in dBFS.
fn silence_floor(threshold_db: f64) -> f64 {
    10f64.powf(threshold_db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_floor_maps_dbfs_to_linear_amplitude() {
        assert!((silence_floor(0.0) - 1.0).abs() < 1e-9);
        assert!((silence_floor(-20.0) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn output_language_is_target_when_translating() {
        let to = Language::parse("en").unwrap();
        let from = Language::parse("pt").unwrap();
        let default = Language::parse("pt-BR").unwrap();
        let out = output_language(RealtimeMode::Translate, &to, Some(&from), &default);
        assert_eq!(out.as_str(), to.as_str());
    }

    #[test]
    fn output_language_is_source_when_not_translating() {
        let to = Language::parse("en").unwrap();
        let from = Language::parse("pt").unwrap();
        let default = Language::parse("ja").unwrap();
        let with_from = output_language(RealtimeMode::NoTranslate, &to, Some(&from), &default);
        assert_eq!(with_from.as_str(), from.as_str());
        let without = output_language(RealtimeMode::Echo, &to, None, &default);
        assert_eq!(without.as_str(), default.as_str());
    }
}
