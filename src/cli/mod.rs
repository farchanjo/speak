//! `cli` driving adapter (T050-T056): the clap surface that maps arguments to
//! application use cases and contains no business logic (ADR-0003).
//!
//! The composition root (`src/main.rs`) builds the concrete [`AppFacade`] object
//! graph (Factory/DI) and dispatches each parsed subcommand to the handler here.
//! Every handler maps CLI arguments to domain value objects, calls the
//! [`speak::application::SpeakFacade`], and renders the result. File I/O (reading
//! input audio, writing `-o`/record output) stays in this driving adapter because
//! the application layer is framework-free and returns owned bytes to persist.

use std::path::Path;

use anyhow::Result;

use speak::adapters::config::Config;
use speak::adapters::coreaudio::CoreAudio;
use speak::adapters::libav::LibavCodec;
use speak::application::{SpeakFacade, StreamTranscribeOptions};
use speak::domain::capture_source::{CaptureDirection, CaptureSource};
use speak::domain::voice::{StandardVoice, VoiceClone, VoiceMode};
use speak::domain::voice_design::VoiceDesign;

use args::{CaptureSourceArg, Command};

pub(crate) mod args;
pub(crate) mod check;
pub(crate) mod completions;
pub(crate) mod config;
pub(crate) mod devices;
pub(crate) mod realtime;
pub(crate) mod record;
pub(crate) mod say;
pub(crate) mod speech;
pub(crate) mod transcribe;
pub(crate) mod translate;
pub(crate) mod voices;

/// The concrete application Facade the composition root injects into every
/// handler: the [`speech::SpeechRole`] selector (in-process retry-wrapped
/// `openai` adapter, or a forwarder to a running warm daemon — T053), the
/// `coreaudio` audio adapter, and the `libav` codec adapter wired together
/// (ADR-0003 / ADR-0005 / T054).
pub(crate) type AppFacade = SpeakFacade<speech::SpeechRole, CoreAudio, LibavCodec>;

/// Extract a multipart-friendly basename from `path`, with a stable fallback.
#[must_use]
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_owned()
}

/// Assemble a [`CaptureSource`] from the shared `--source`/`-d`/`-I` flags with
/// the config catalog (ADR-0015), honoring `flag > toml > default`:
///
/// - direction: the `--source` flag, else `[audio.capture].source` (default
///   `input`);
/// - device (`0` = unset on the flag): the flag, else — for the output source —
///   `[audio.capture].device`;
/// - channel: the `-I` flag, else `[audio.capture].channel` (output) or
///   `[audio.input].channel` (input).
#[must_use]
pub(crate) fn capture_source(
    flag_source: Option<CaptureSourceArg>,
    device: u32,
    flag_channel: Option<u16>,
    cfg: &Config,
) -> CaptureSource {
    let direction = flag_source.map_or_else(
        || cfg.audio.capture.direction(),
        CaptureSourceArg::direction,
    );
    let device = (device != 0).then_some(device).or_else(|| {
        matches!(direction, CaptureDirection::Output)
            .then_some(cfg.audio.capture.device)
            .flatten()
    });
    let channel = flag_channel.or(match direction {
        CaptureDirection::Output => cfg.audio.capture.channel,
        CaptureDirection::Input => cfg.audio.input.channel,
    });
    CaptureSource::new(direction, device, channel)
}

/// Resolve the shared live-streaming options (capture source + chunk + VAD) from
/// the common `transcribe --stream` / `translate --stream` flags + config
/// (ADR-0014/0017). `[audio.input]` supplies the chunk/VAD defaults.
#[must_use]
pub(crate) fn stream_options(
    source: Option<CaptureSourceArg>,
    device: u32,
    input_channel: Option<u16>,
    chunk: u64,
    no_vad: bool,
    vad_floor: Option<f64>,
    cfg: &Config,
) -> StreamTranscribeOptions {
    let chunk_secs = if chunk == 5 {
        cfg.audio.input.chunk_secs
    } else {
        f64::from(chunk as u32)
    };
    let threshold_db = vad_floor.unwrap_or(cfg.audio.input.silence_threshold_db);
    StreamTranscribeOptions {
        source: capture_source(source, device, input_channel, cfg),
        chunk_secs,
        vad: cfg.audio.input.vad && !no_vad,
        silence_floor: 10f64.powf(threshold_db / 20.0),
    }
}

/// Whether `command` will capture the host **output** — the source resolves to
/// `output` via the `--source` flag or `[audio.capture].source`. The composition
/// root uses this to decide the macOS TCC-disclaim re-exec (ADR-0016).
#[must_use]
pub(crate) fn wants_output_capture(command: &Command, cfg: &Config) -> bool {
    let source = match command {
        Command::Transcribe(a) if a.stream => a.source,
        Command::Translate(a) if a.stream => a.source,
        Command::Record(a) => a.source,
        Command::Realtime(a) => a.source,
        _ => return false,
    };
    let direction = source.map_or_else(
        || cfg.audio.capture.direction(),
        CaptureSourceArg::direction,
    );
    matches!(direction, CaptureDirection::Output)
}

/// Resolve the voice **Strategy** (FR-2) shared by `say` and `realtime`:
/// `instruct` tags select the voice-design arm; an `explicit_voice` flag or a
/// `ref_text` selects the clone arm (carrying the reference transcript); else the
/// configured `voice_name` is the standard voice.
pub(crate) fn resolve_voice(
    voice_name: &str,
    explicit_voice: bool,
    instruct: Option<&str>,
    ref_text: Option<&str>,
) -> Result<VoiceMode> {
    if let Some(tags) = instruct {
        Ok(VoiceMode::Design(VoiceDesign::parse(tags)?))
    } else if explicit_voice || ref_text.is_some() {
        Ok(VoiceMode::Clone(VoiceClone::new(voice_name, ref_text)?))
    } else {
        Ok(VoiceMode::Standard(StandardVoice::new(voice_name)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_extracts_basename_with_fallback() {
        assert_eq!(file_name(Path::new("/a/b/clip.wav")), "clip.wav");
        assert_eq!(file_name(Path::new("plain.mp3")), "plain.mp3");
        assert_eq!(file_name(Path::new("/")), "audio");
    }
}
