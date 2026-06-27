//! `realtime` handler: capture the microphone and translate it live (FR-8).
//!
//! Runs the chunked ASR -> (MT) -> TTS loop over the request transport until
//! Ctrl-C: capture one chunk, resample to 16 kHz mono, gate silence, translate
//! or transcribe it, print the text, and optionally re-voice it. The SSE path and
//! the use-case-driven realtime modes land with the `sse` adapter (T036/T044).

use anyhow::Result;

use speak::adapters::{coreaudio, libav};
use speak::client::{self, SpeakRequest};
use speak::config::Config;
use speak::domain::voice_design::VoiceDesign;
use speak::transport::Transport;

use super::args::{GlobalArgs, RealtimeArgs};
use super::say::gen_to_map;

/// Run the `realtime` subcommand loop.
pub async fn run(cfg: &Config, globals: &GlobalArgs, args: RealtimeArgs) -> Result<()> {
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
            res = iterate(&transport, cfg, &args, chunk, globals.quiet) => {
                if let Err(e) = res { eprintln!("warn: {e:#}"); }
            }
        }
    }
}

/// Capture, gate, translate, print, and optionally re-voice one chunk.
async fn iterate(
    transport: &Transport,
    cfg: &Config,
    args: &RealtimeArgs,
    chunk: f64,
    quiet: bool,
) -> Result<()> {
    let device = args.device;
    let pcm =
        tokio::task::spawn_blocking(move || coreaudio::capture_chunk(device, chunk)).await??;
    let mono = tokio::task::spawn_blocking(move || libav::to_asr_mono16(&pcm)).await??;
    if cfg.audio.input.vad && libav::rms_s16(&mono) < silence_floor(cfg) {
        return Ok(());
    }
    let wav = libav::wav_mono16(&mono, libav::ASR_RATE);
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

/// Multipart ASR fields (model + response format, optional language hint).
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

/// Transcribe one chunk in the source language.
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

/// Translate one chunk, picking transcribe / Whisper-English / chat-MT.
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

/// Translate `text` into `target` via the configured chat-completions endpoint.
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

/// Synthesize and play `text` in the configured output voice.
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

/// Validate an optional voice-design instruct string against the canonical tags.
fn validate_instruct(instruct: Option<&str>) -> Result<Option<String>> {
    instruct
        .map(|raw| VoiceDesign::parse(raw).map(|d| d.instruct()))
        .transpose()
}

/// Decode encoded audio and play it through the native CoreAudio mixer.
async fn play_bytes(bytes: Vec<u8>, content_type: String, cfg: &Config, quiet: bool) -> Result<()> {
    let opts = libav::DecodeOptions {
        threads: cfg.ffmpeg.threads,
        log_level: cfg.ffmpeg.log_level.clone(),
    };
    let volume = cfg.audio.output.volume;
    let (samples, frames, secs) = tokio::task::spawn_blocking(move || -> Result<_> {
        let pcm = libav::decode(bytes, &opts)?;
        let stats = (pcm.samples().len(), pcm.frames(), pcm.duration_secs());
        coreaudio::play(&pcm, volume)?;
        Ok(stats)
    })
    .await??;
    if !quiet {
        eprintln!(
            "decoded {content_type}: {samples} samples ({frames} frames @ {}Hz, {secs:.2}s); \
             played via native CoreAudio mixer",
            libav::PLAY_RATE
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asr_fields_omit_language_when_none() {
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
        let fields = asr_fields("whisper-1", Some("pt"), "text");
        assert!(fields.contains(&("language".to_owned(), "pt".to_owned())));
        assert_eq!(fields.len(), 3);
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
}
