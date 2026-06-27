//! `realtime` handler (T051): capture the microphone and re-voice it live (FR-8).
//!
//! A thin driving adapter: it maps the CLI flags to a [`RealtimeOptions`] value
//! object, drives a **continuous** capture stream (ADR-0017) and runs the
//! application [`RealtimeUseCase`] (via the [`AppFacade`]) once per captured
//! chunk until Ctrl-C. Capture never pauses while a chunk is processed, so a slow
//! round trip does not drop audio. The use case owns the ASR -> (MT) -> re-voice
//! pipeline and the playback routing; this handler resolves options, owns the
//! capture stream, surfaces each caption through the [`Presenter`], and sends
//! loop diagnostics to `tracing`. The pipeline mode is the exclusive
//! `--translate` (default) / `--no-translate` / `--echo` group; the spoken output
//! voice is `--instruct` (design) or the global `--voice` (clone / standard).

use anyhow::Result;

use speak::adapters::config::Config;
use speak::adapters::coreaudio::{CoreAudio, NativeCaptureStream};
use speak::adapters::sse::{RealtimeRequest, SseRealtimeClient};
use speak::application::{RealtimeEvent, RealtimeOptions, RealtimeStep};
use speak::domain::audio_format::AudioFormat;
use speak::domain::language::Language;
use speak::domain::pcm::PcmBuffer;
use speak::domain::realtime::RealtimeMode;
use speak::domain::voice::VoiceMode;
use speak::ports::audio::AudioDeviceId;
use speak::ports::presenter::Presenter;

use super::AppFacade;
use super::args::{GlobalArgs, RealtimeArgs};
use super::say::gen_to_params;

/// Advertised file name for a captured realtime chunk.
const CHUNK_NAME: &str = "chunk.wav";

/// Run the `realtime` subcommand loop until Ctrl-C.
///
/// A [`ServerProbe`](speak::ports::probe::ServerProbe) capability check selects
/// the SSE path (`POST /v1/realtime/translate`) when the endpoint exists, else the
/// chunked ASR -> MT -> TTS fallback (ADR-0004). One prebuilt binary decides at
/// run time; both paths share the resolved [`RealtimeOptions`].
pub(crate) async fn run(
    facade: &AppFacade,
    cfg: &Config,
    globals: &GlobalArgs,
    args: RealtimeArgs,
    sse: &SseRealtimeClient,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let opts = build_options(cfg, globals, &args)?;
    let realtime = facade.supports_realtime().await.unwrap_or(false);
    // Continuous capture (ADR-0017): one producer feeds chunks; the loop is the
    // consumer, so a slow round trip never pauses capture (no dropped words).
    let mut capture = CoreAudio::new().capture_stream(
        &opts.source,
        opts.chunk_secs,
        cfg.audio.capture.buffer_secs,
    )?;
    tracing::info!(
        mode = opts.mode.as_str(),
        chunk = opts.chunk_secs,
        device = args.device,
        source = opts.source.direction().as_str(),
        from = opts.from.as_ref().map_or("auto", Language::as_str),
        to = opts.to.as_str(),
        path = if realtime { "sse" } else { "chunked" },
        "realtime loop starting; Ctrl-C to stop"
    );
    if realtime {
        run_sse(facade, cfg, sse, &mut capture, &opts, presenter).await
    } else {
        run_chunked(facade, &mut capture, &opts, presenter).await
    }
}

/// The chunked fallback: consume captured chunks -> ASR -> (MT) -> re-voice.
async fn run_chunked(
    facade: &AppFacade,
    capture: &mut NativeCaptureStream,
    opts: &RealtimeOptions,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("realtime loop stopping");
                return Ok(());
            }
            chunk = capture.recv() => {
                let Some(raw) = chunk else {
                    tracing::warn!("capture stream ended");
                    return Ok(());
                };
                match facade.realtime_process(raw, opts).await {
                    Ok(Some(step)) => emit_step(&step, presenter)?,
                    Ok(None) => {}
                    Err(e) => tracing::warn!("realtime chunk failed: {e:#}"),
                }
            }
        }
    }
}

/// The SSE path: POST each captured chunk and play back the streamed frames.
async fn run_sse(
    facade: &AppFacade,
    cfg: &Config,
    sse: &SseRealtimeClient,
    capture: &mut NativeCaptureStream,
    opts: &RealtimeOptions,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("realtime loop stopping");
                return Ok(());
            }
            chunk = capture.recv() => {
                let Some(raw) = chunk else {
                    tracing::warn!("capture stream ended");
                    return Ok(());
                };
                if let Err(e) = drive_chunk(facade, cfg, sse, opts, raw, presenter).await {
                    tracing::warn!("realtime SSE chunk failed: {e:#}");
                }
            }
        }
    }
}

/// Encode one captured chunk, stream it through the SSE endpoint, play the frames.
async fn drive_chunk(
    facade: &AppFacade,
    cfg: &Config,
    sse: &SseRealtimeClient,
    opts: &RealtimeOptions,
    raw: PcmBuffer,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let Some(wav) = facade.realtime_encode(raw, opts)? else {
        return Ok(());
    };
    let request = realtime_request(wav, opts);
    let mut stream = sse.stream(request, cfg.retry.policy, cfg.retry.jitter_seed);
    let mut emit_err = None;
    facade
        .realtime_drive(&mut stream, opts, |event| {
            on_event(event, presenter, &mut emit_err);
        })
        .await?;
    emit_err.map_or(Ok(()), Err)
}

/// Surface one streamed event: print text, log a server error, ignore playback.
fn on_event(
    event: &RealtimeEvent,
    presenter: &mut dyn Presenter,
    emit_err: &mut Option<anyhow::Error>,
) {
    match event {
        RealtimeEvent::Text { text, .. } => {
            if let Err(e) = presenter.line(text) {
                emit_err.get_or_insert(e);
            }
        }
        RealtimeEvent::Failed { message } => tracing::warn!("realtime server error: {message}"),
        RealtimeEvent::Played | RealtimeEvent::Done => {}
    }
}

/// Project the resolved options + captured chunk onto the SSE request fields.
fn realtime_request(wav: Vec<u8>, opts: &RealtimeOptions) -> RealtimeRequest {
    let (voice, instruct) = voice_fields(&opts.voice);
    RealtimeRequest {
        audio: wav,
        filename: CHUNK_NAME.to_owned(),
        to: Some(opts.to.as_str().to_owned()),
        translate: matches!(opts.mode, RealtimeMode::Translate),
        voice,
        instruct,
        language: opts.from.as_ref().map(|l| l.as_str().to_owned()),
        format: opts.format.as_str().to_owned(),
    }
}

/// Map the output [`VoiceMode`] Strategy onto the `voice`/`instruct` form fields.
fn voice_fields(mode: &VoiceMode) -> (Option<String>, Option<String>) {
    match mode {
        VoiceMode::Design(design) => (None, Some(design.instruct())),
        VoiceMode::Clone(clone) => (Some(clone.name().to_owned()), None),
        VoiceMode::Standard(voice) => (Some(voice.name().to_owned()), None),
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
        gen_params: gen_to_params(&cfg.tts.gen_params),
        chunk_secs: chunk_secs(cfg, args),
        source: super::capture_source(args.source, args.device, args.input_channel, cfg),
        outputs: args
            .output_device
            .iter()
            .copied()
            .map(AudioDeviceId)
            .collect(),
        volume: cfg.audio.output.volume,
        vad: cfg.audio.input.vad && !args.no_vad,
        silence_floor: silence_floor(
            args.vad_floor
                .unwrap_or(cfg.audio.input.silence_threshold_db),
        ),
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
