//! `say` handler (T051): synthesize speech and optionally play / save it.
//!
//! Maps the `say` arguments to a validated [`SpeechSpec`] and drives the
//! [`AppFacade`]'s `say` use case (FR-1 / FR-11). Reading stdin and writing the
//! `-o` file stay here in the driving adapter; the use case returns the encoded
//! bytes plus the server's `X-RTF`/`X-Audio-Seconds` timing headers.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use speak::adapters::config::{self, Config};
use speak::application::SayOptions;
use speak::domain::audio_format::AudioFormat;
use speak::domain::language::Language;
use speak::domain::{gen_params, speech_spec::SpeechSpec, voice_design};
use speak::ports::audio::AudioDeviceId;
use speak::ports::presenter::{Presenter, Report, Table};
use speak::ports::synthesizer::SynthesizedAudio;

use super::AppFacade;
use super::args::{GlobalArgs, SayArgs};

/// Run the `say` subcommand, emitting its RESULT through the Presenter.
pub async fn run(
    facade: &AppFacade,
    cfg: &Config,
    globals: &GlobalArgs,
    args: SayArgs,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    if args.list_designs {
        return list_designs(presenter);
    }
    let text = resolve_text(&args.text)?;
    let format = args
        .format
        .map_or_else(|| cfg.tts.format.clone(), |f| f.as_str().to_owned());
    let spec = build_spec(cfg, globals, &args, &text, &format)?;
    let opts = SayOptions {
        play: !args.no_play && cfg.audio.output.play,
        volume: cfg.audio.output.volume,
        devices: args
            .output_device
            .iter()
            .copied()
            .map(AudioDeviceId)
            .collect(),
    };
    let outcome = facade.say(&spec, &opts).await?;
    let saved = match &args.out {
        Some(path) => Some(write_output(cfg, path, &outcome.audio.bytes).await?),
        None => None,
    };
    report(&outcome.audio, saved.as_deref(), presenter)
}

/// Emit the canonical voice-design tags as a table and exit (no network).
fn list_designs(presenter: &mut dyn Presenter) -> Result<()> {
    let mut table = Table::new(["voice-design tag"]);
    for tag in voice_design::CANONICAL_TAGS {
        table = table.row([(*tag).to_owned()]);
    }
    presenter.table(&table)
}

/// Persist the synthesized bytes to the resolved `-o` path, returning that path.
async fn write_output(cfg: &Config, path: &Path, bytes: &[u8]) -> Result<String> {
    let path = resolve_out_path(cfg, path);
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(bytes = bytes.len(), path = %path.display(), "saved synthesized audio");
    Ok(path.display().to_string())
}

/// Resolve a bare `-o` filename under `[http].save_dir` (FR-1).
fn resolve_out_path(cfg: &Config, path: &Path) -> PathBuf {
    match (
        &cfg.http.save_dir,
        path.is_absolute() || path.parent().is_some_and(|p| !p.as_os_str().is_empty()),
    ) {
        (Some(dir), false) => dir.join(path),
        _ => path.to_path_buf(),
    }
}

/// Assemble the validated [`SpeechSpec`] from the `say` args + config.
///
/// Resolves the voice **Strategy** (FR-2): `--instruct` (or `[tts].instruct`)
/// selects the voice-design arm; an explicit `--voice`/`SPEAK_VOICE` or a
/// `--ref-text` selects the clone arm (carrying the optional reference
/// transcript); otherwise the configured `[tts].voice` is the standard voice.
fn build_spec(
    cfg: &Config,
    globals: &GlobalArgs,
    args: &SayArgs,
    text: &str,
    format: &str,
) -> Result<SpeechSpec> {
    let instruct = args.instruct.as_deref().or(cfg.tts.instruct.as_deref());
    let voice = super::resolve_voice(
        &cfg.tts.voice,
        globals.voice.is_some(),
        instruct,
        args.ref_text.as_deref(),
    )?;
    Ok(SpeechSpec::builder(text)
        .voice(voice)
        .language(Language::parse(&cfg.tts.language)?)
        .format(AudioFormat::parse(format)?)
        .speed(args.speed)
        .gen_params(gen_extra(cfg, &args.set)?)
        .build()?)
}

/// Emit the synthesis RESULT (timing metadata + byte count + save path) through
/// the Presenter so `--json` surfaces the server's `X-RTF`/`X-Audio-Seconds`
/// (FR-1); the console renderer suppresses it under `--quiet`.
fn report(
    audio: &SynthesizedAudio,
    saved: Option<&str>,
    presenter: &mut dyn Presenter,
) -> Result<()> {
    let mut report = Report::titled("synthesis");
    if let Some(secs) = &audio.audio_seconds {
        report = report.entry("audio_seconds", secs.as_str());
    }
    if let Some(rtf) = &audio.rtf {
        report = report.entry("rtf", rtf.as_str());
    }
    report = report.entry("bytes", audio.bytes.len().to_string());
    if let Some(path) = saved {
        report = report.entry("saved", path);
    }
    presenter.report(&report)
}

/// Merge configured `[tts.gen]` params, then overlay per-call `--set` overrides.
pub fn gen_extra(cfg: &Config, sets: &[String]) -> Result<Map<String, Value>> {
    let mut map = gen_to_map(&cfg.tts.gen_params);
    for (key, value) in gen_params::parse_overrides(sets)? {
        map.insert(key, value);
    }
    Ok(map)
}

/// Project the configured `[tts.gen]` params into a JSON map (unset => absent).
pub fn gen_to_map(g: &config::Gen) -> Map<String, Value> {
    use serde_json::json;
    let mut m = Map::new();
    let mut put = |k: &str, v: Option<Value>| {
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

/// Join positional text args, or read stdin when none were given.
pub fn resolve_text(parts: &[String]) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gen_to_map_only_emits_set_params() {
        let params = config::Gen {
            num_step: Some(24),
            guidance_scale: Some(3.0),
            denoise: Some(true),
            ..config::Gen::default()
        };
        let map = gen_to_map(&params);
        assert_eq!(map.get("num_step"), Some(&json!(24)));
        assert_eq!(map.get("guidance_scale"), Some(&json!(3.0)));
        assert_eq!(map.get("denoise"), Some(&json!(true)));
        assert!(!map.contains_key("t_shift"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn gen_to_map_empty_when_nothing_set() {
        assert!(gen_to_map(&config::Gen::default()).is_empty());
    }

    #[test]
    fn resolve_text_joins_positional_args() {
        let parts = vec!["hello".to_owned(), "world".to_owned()];
        assert_eq!(resolve_text(&parts).unwrap(), "hello world");
    }
}
