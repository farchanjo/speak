//! `realtime` use case (T044): the live microphone pipeline (FR-8).
//!
//! Drives the three [`RealtimeMode`] **Strategy** arms over the driven ports,
//! grouped by adapter role — `Speech` (synthesize/transcribe/translate), `Audio`
//! (capture/play), `Codec` (resample/encode). The driving adapter owns the
//! Ctrl-C loop and the terminal output; each call to [`step`] runs exactly one
//! chunk so the use case stays free of any framework type:
//!
//! - `Translate`: ASR -> MT (the `Translator` Strategy picks Whisper-English or
//!   chat-MT), then re-voice + play the translation.
//! - `NoTranslate`: passthrough re-voice (ASR -> TTS in the chosen output voice).
//! - `Echo`: play the raw capture, then re-voice it.
//!
//! When the server advertises the SSE endpoint the composition root drives the
//! `RealtimeStream` port instead and feeds each frame to [`pump_frame`]; both
//! paths share the same playback routing (single device or fan-out, FR-11).
//!
//! [`step`]: RealtimeUseCase::step
//! [`pump_frame`]: RealtimeUseCase::pump_frame

use anyhow::Result;

use crate::application::playback;
use crate::domain::audio_format::AudioFormat;
use crate::domain::gen_params::GenParams;
use crate::domain::language::Language;
use crate::domain::pcm::PcmBuffer;
use crate::domain::realtime::RealtimeMode;
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::VoiceMode;
use crate::ports::audio::{AudioDeviceId, AudioSink, AudioSource};
use crate::ports::codec::{AudioDecoder, AudioEncoder, RecordFormat};
use crate::ports::realtime::{RealtimeFrame, RealtimeStream};
use crate::ports::synthesizer::Synthesizer;
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;

/// Whisper's required ASR sample rate (Hz) — a fixed protocol constant.
const ASR_RATE: u32 = 16_000;
/// Whisper expects mono audio.
const ASR_CHANNELS: u16 = 1;
/// Upload file name advertised to the ASR/translate endpoints.
const CHUNK_NAME: &str = "chunk.wav";

/// Per-chunk options for a realtime invocation.
#[derive(Debug, Clone)]
pub struct RealtimeOptions {
    /// The pipeline strategy.
    pub mode: RealtimeMode,
    /// Source-language hint for ASR (`None` = auto-detect).
    pub from: Option<Language>,
    /// Translation target language.
    pub to: Language,
    /// Output voice for re-voicing (design / clone / standard).
    pub voice: VoiceMode,
    /// Language the re-voiced output is spoken in.
    pub output_language: Language,
    /// TTS output format.
    pub format: AudioFormat,
    /// TTS speed multiplier.
    pub speed: f32,
    /// Pass-through generation params for the TTS request.
    pub gen_params: GenParams,
    /// Capture chunk length in seconds.
    pub chunk_secs: f64,
    /// Capture device (`None` = system default input).
    pub device: Option<AudioDeviceId>,
    /// Select one 0-based input channel before the mono downmix (`None` keeps the
    /// downmix of all channels, ADR-0013) — for a mic on one input of a
    /// multi-channel interface.
    pub input_channel: Option<u16>,
    /// Playback output devices; empty = default (fan-out when > 1, FR-11).
    pub outputs: Vec<AudioDeviceId>,
    /// Mixer volume.
    pub volume: f32,
    /// Whether the silence (VAD) gate is enabled.
    pub vad: bool,
    /// Linear RMS floor below which a chunk is treated as silence.
    pub silence_floor: f64,
}

/// The outcome of one processed chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeStep {
    /// Source-language transcript (no-translate / echo modes).
    pub source_text: Option<String>,
    /// The text that was synthesized and played.
    pub output_text: String,
    /// Whether re-voiced audio was played.
    pub spoken: bool,
}

/// The kind of text carried by an SSE frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// A source-language transcript frame.
    Transcript,
    /// A target-language translation frame.
    Translation,
}

/// The result of pumping one SSE frame (the driving adapter prints / loops).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeEvent {
    /// Text to surface to the user.
    Text {
        /// Whether the text is a transcript or a translation.
        kind: FrameKind,
        /// The frame text.
        text: String,
    },
    /// A synthesized audio chunk was decoded and played.
    Played,
    /// The stream completed normally.
    Done,
    /// The server reported an error mid-stream.
    Failed {
        /// The error message.
        message: String,
    },
}

/// The realtime use case over the speech, audio, and codec port roles.
pub struct RealtimeUseCase<'a, Speech, Audio, Codec> {
    speech: &'a Speech,
    audio: &'a Audio,
    codec: &'a Codec,
}

impl<'a, Speech, Audio, Codec> RealtimeUseCase<'a, Speech, Audio, Codec>
where
    Speech: Synthesizer + Transcriber + Translator,
    Audio: AudioSource + AudioSink,
    Codec: AudioDecoder + AudioEncoder,
{
    /// Wire the use case to its port roles.
    #[must_use]
    pub fn new(speech: &'a Speech, audio: &'a Audio, codec: &'a Codec) -> Self {
        Self {
            speech,
            audio,
            codec,
        }
    }

    /// Capture and process one chunk; `Ok(None)` = silence or empty result.
    pub async fn step(&self, opts: &RealtimeOptions) -> Result<Option<RealtimeStep>> {
        let Some((captured, wav)) = self.capture_gated(opts).await? else {
            return Ok(None);
        };
        match opts.mode {
            RealtimeMode::Translate => self.translate_step(&wav, opts).await,
            RealtimeMode::NoTranslate => self.revoice_step(&wav, opts).await,
            RealtimeMode::Echo => self.echo_step(&captured, &wav, opts).await,
        }
    }

    /// Capture one chunk and encode it to WAV for the SSE endpoint (T036).
    ///
    /// `Ok(None)` when the silence gate suppresses the chunk. The server runs the
    /// ASR -> MT -> TTS pipeline and streams the result back as frames, so this
    /// path only needs the encoded capture, not local re-voicing.
    pub async fn capture_chunk(&self, opts: &RealtimeOptions) -> Result<Option<Vec<u8>>> {
        Ok(self.capture_gated(opts).await?.map(|(_, wav)| wav))
    }

    /// Capture one chunk, resample to the ASR rate, gate silence, and mux WAV.
    ///
    /// Returns the raw capture (for echo playback) alongside the encoded WAV, or
    /// `Ok(None)` when the VAD gate treats the chunk as silence.
    async fn capture_gated(&self, opts: &RealtimeOptions) -> Result<Option<(PcmBuffer, Vec<u8>)>> {
        let captured = self.audio.capture(opts.device, opts.chunk_secs).await?;
        let captured = super::pick_input_channel(captured, opts.input_channel)?;
        let mono = self.codec.resample(&captured, ASR_RATE, ASR_CHANNELS)?;
        if opts.vad && rms(&mono) < opts.silence_floor {
            return Ok(None);
        }
        let wav = self.codec.encode(&mono, RecordFormat::Wav)?;
        Ok(Some((captured, wav)))
    }

    /// Decode and play one SSE realtime frame, or surface its text/terminal state.
    pub async fn pump_frame(
        &self,
        frame: RealtimeFrame,
        opts: &RealtimeOptions,
    ) -> Result<RealtimeEvent> {
        match frame {
            RealtimeFrame::Transcript { text } => Ok(text_event(FrameKind::Transcript, text)),
            RealtimeFrame::Translation { text } => Ok(text_event(FrameKind::Translation, text)),
            RealtimeFrame::Audio { data, .. } => {
                self.play_bytes(&data, opts).await?;
                Ok(RealtimeEvent::Played)
            }
            RealtimeFrame::Done => Ok(RealtimeEvent::Done),
            RealtimeFrame::Error { message } => Ok(RealtimeEvent::Failed { message }),
        }
    }

    /// Consume an SSE stream to completion, invoking `on_event` per frame.
    pub async fn drive_stream<St, F>(
        &self,
        stream: &mut St,
        opts: &RealtimeOptions,
        mut on_event: F,
    ) -> Result<()>
    where
        St: RealtimeStream,
        F: FnMut(&RealtimeEvent),
    {
        while let Some(frame) = stream.recv().await? {
            let event = self.pump_frame(frame, opts).await?;
            let stop = matches!(event, RealtimeEvent::Done | RealtimeEvent::Failed { .. });
            on_event(&event);
            if stop {
                break;
            }
        }
        Ok(())
    }

    /// Translate mode: ASR -> MT, then re-voice the translation.
    async fn translate_step(
        &self,
        wav: &[u8],
        opts: &RealtimeOptions,
    ) -> Result<Option<RealtimeStep>> {
        let text = self.speech.translate(wav, CHUNK_NAME, &opts.to).await?;
        self.finish(None, text, opts).await
    }

    /// No-translate mode: transcribe, then re-voice the transcript.
    async fn revoice_step(
        &self,
        wav: &[u8],
        opts: &RealtimeOptions,
    ) -> Result<Option<RealtimeStep>> {
        let text = self.transcribe(wav, opts).await?;
        self.finish(Some(text.clone()), text, opts).await
    }

    /// Echo mode: play the raw capture, then transcribe + re-voice it.
    async fn echo_step(
        &self,
        raw: &PcmBuffer,
        wav: &[u8],
        opts: &RealtimeOptions,
    ) -> Result<Option<RealtimeStep>> {
        playback::play_pcm(self.audio, raw, &opts.outputs, opts.volume).await?;
        let text = self.transcribe(wav, opts).await?;
        self.finish(Some(text.clone()), text, opts).await
    }

    /// Transcribe `wav` with the configured source-language hint.
    async fn transcribe(&self, wav: &[u8], opts: &RealtimeOptions) -> Result<String> {
        let req = TranscribeRequest {
            audio: wav,
            filename: CHUNK_NAME,
            language: opts.from.as_ref(),
            format: "json",
        };
        self.speech.transcribe(&req).await
    }

    /// Re-voice `text` and play it, unless it is empty.
    async fn finish(
        &self,
        source: Option<String>,
        text: String,
        opts: &RealtimeOptions,
    ) -> Result<Option<RealtimeStep>> {
        if text.trim().is_empty() {
            return Ok(None);
        }
        self.speak_and_play(&text, opts).await?;
        Ok(Some(RealtimeStep {
            source_text: source,
            output_text: text,
            spoken: true,
        }))
    }

    /// Synthesize `text` in the output voice and route it to the sinks.
    async fn speak_and_play(&self, text: &str, opts: &RealtimeOptions) -> Result<()> {
        let spec = SpeechSpec::builder(text)
            .voice(opts.voice.clone())
            .language(opts.output_language.clone())
            .format(opts.format)
            .speed(opts.speed)
            .gen_params(opts.gen_params.clone())
            .build()?;
        let audio = self.speech.synthesize(&spec).await?;
        self.play_bytes(&audio.bytes, opts).await
    }

    /// Decode encoded audio bytes and play them on the configured sinks.
    async fn play_bytes(&self, bytes: &[u8], opts: &RealtimeOptions) -> Result<()> {
        playback::decode_and_play(self.codec, self.audio, bytes, &opts.outputs, opts.volume)
            .await?;
        Ok(())
    }
}

/// Build a text SSE event.
fn text_event(kind: FrameKind, text: String) -> RealtimeEvent {
    RealtimeEvent::Text { kind, text }
}

/// Linear RMS amplitude of an interleaved float buffer (silence gate input).
fn rms(pcm: &PcmBuffer) -> f64 {
    let samples = pcm.samples();
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    (sum / samples.len() as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec, FakeSpeech};
    use crate::domain::voice::StandardVoice;

    fn opts(mode: RealtimeMode) -> RealtimeOptions {
        RealtimeOptions {
            mode,
            from: None,
            to: Language::parse("en").unwrap(),
            voice: VoiceMode::Standard(StandardVoice::new("alloy").unwrap()),
            output_language: Language::parse("en").unwrap(),
            format: AudioFormat::Mp3,
            speed: 1.0,
            gen_params: GenParams::new(),
            chunk_secs: 5.0,
            device: None,
            input_channel: None,
            outputs: Vec::new(),
            volume: 1.0,
            vad: false,
            silence_floor: 0.1,
        }
    }

    #[tokio::test]
    async fn translate_mode_revoices_the_translation() {
        let speech = FakeSpeech {
            translation: "hello world".to_owned(),
            ..FakeSpeech::default()
        };
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let step = RealtimeUseCase::new(&speech, &audio, &codec)
            .step(&opts(RealtimeMode::Translate))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(step.output_text, "hello world");
        assert!(step.source_text.is_none());
        assert!(step.spoken);
        assert_eq!(audio.plays.lock().unwrap().len(), 1, "one TTS playback");
    }

    #[tokio::test]
    async fn no_translate_mode_revoices_the_transcript() {
        let speech = FakeSpeech {
            transcript: "bom dia".to_owned(),
            ..FakeSpeech::default()
        };
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let step = RealtimeUseCase::new(&speech, &audio, &codec)
            .step(&opts(RealtimeMode::NoTranslate))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(step.source_text.as_deref(), Some("bom dia"));
        assert_eq!(step.output_text, "bom dia");
        assert_eq!(audio.plays.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn echo_mode_plays_raw_then_revoices() {
        let speech = FakeSpeech {
            transcript: "echo me".to_owned(),
            ..FakeSpeech::default()
        };
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        RealtimeUseCase::new(&speech, &audio, &codec)
            .step(&opts(RealtimeMode::Echo))
            .await
            .unwrap()
            .unwrap();
        // Two playbacks: raw capture, then the re-voiced audio.
        assert_eq!(audio.plays.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn vad_skips_silent_chunks_but_passes_speech() {
        let codec = FakeCodec;
        let speech = FakeSpeech::default();
        let mut o = opts(RealtimeMode::Translate);
        o.vad = true;

        let silent = FakeAudio {
            capture_pcm: PcmBuffer::new(vec![0.0; 4_800], 48_000, 2),
            ..FakeAudio::default()
        };
        let skipped = RealtimeUseCase::new(&speech, &silent, &codec)
            .step(&o)
            .await
            .unwrap();
        assert!(skipped.is_none(), "silence is gated");
        assert!(silent.plays.lock().unwrap().is_empty());

        let loud = FakeAudio::default(); // 0.5 amplitude
        let passed = RealtimeUseCase::new(&speech, &loud, &codec)
            .step(&o)
            .await
            .unwrap();
        assert!(passed.is_some(), "speech passes the gate");
    }

    #[tokio::test]
    async fn empty_result_yields_no_step() {
        let speech = FakeSpeech {
            translation: "   ".to_owned(),
            ..FakeSpeech::default()
        };
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let step = RealtimeUseCase::new(&speech, &audio, &codec)
            .step(&opts(RealtimeMode::Translate))
            .await
            .unwrap();
        assert!(step.is_none());
        assert!(audio.plays.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pump_frame_plays_audio_and_surfaces_text() {
        let speech = FakeSpeech::default();
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = RealtimeUseCase::new(&speech, &audio, &codec);
        let o = opts(RealtimeMode::Translate);

        let played = uc
            .pump_frame(
                RealtimeFrame::Audio {
                    data: b"AUDIO".to_vec(),
                    format: Some("mp3".to_owned()),
                    seq: Some(1),
                },
                &o,
            )
            .await
            .unwrap();
        assert_eq!(played, RealtimeEvent::Played);
        assert_eq!(audio.plays.lock().unwrap().len(), 1);

        let text = uc
            .pump_frame(
                RealtimeFrame::Translation {
                    text: "hi".to_owned(),
                },
                &o,
            )
            .await
            .unwrap();
        assert_eq!(
            text,
            RealtimeEvent::Text {
                kind: FrameKind::Translation,
                text: "hi".to_owned()
            }
        );
        assert_eq!(
            uc.pump_frame(RealtimeFrame::Done, &o).await.unwrap(),
            RealtimeEvent::Done
        );
    }

    #[tokio::test]
    async fn drive_stream_pumps_each_frame_and_stops_on_done() {
        use crate::application::fakes::FakeStream;

        let speech = FakeSpeech::default();
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = RealtimeUseCase::new(&speech, &audio, &codec);
        let o = opts(RealtimeMode::Translate);
        // Transcript -> Translation -> Audio -> Done; the frame after Done must be
        // unreachable because the loop breaks on the terminal frame.
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Transcript { text: "uno".into() },
            RealtimeFrame::Translation { text: "one".into() },
            RealtimeFrame::Audio {
                data: b"AUDIO".to_vec(),
                format: Some("mp3".into()),
                seq: Some(1),
            },
            RealtimeFrame::Done,
            RealtimeFrame::Translation {
                text: "unreached".into(),
            },
        ]);

        let mut events = Vec::new();
        uc.drive_stream(&mut stream, &o, |e| events.push(e.clone()))
            .await
            .unwrap();

        assert_eq!(events.len(), 4, "stops on Done, never pumps the 5th frame");
        assert_eq!(
            events[0],
            RealtimeEvent::Text {
                kind: FrameKind::Transcript,
                text: "uno".into()
            }
        );
        assert_eq!(events[1], text_event(FrameKind::Translation, "one".into()));
        assert_eq!(events[2], RealtimeEvent::Played);
        assert_eq!(events[3], RealtimeEvent::Done);
        assert_eq!(
            audio.plays.lock().unwrap().len(),
            1,
            "exactly one decoded audio chunk played"
        );
    }

    #[tokio::test]
    async fn drive_stream_stops_on_a_server_error_frame() {
        use crate::application::fakes::FakeStream;

        let speech = FakeSpeech::default();
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = RealtimeUseCase::new(&speech, &audio, &codec);
        let o = opts(RealtimeMode::Translate);
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Error {
                message: "boom".into(),
            },
            RealtimeFrame::Done,
        ]);

        let mut events = Vec::new();
        uc.drive_stream(&mut stream, &o, |e| events.push(e.clone()))
            .await
            .unwrap();

        assert_eq!(events.len(), 1, "a Failed frame is terminal");
        assert_eq!(
            events[0],
            RealtimeEvent::Failed {
                message: "boom".into()
            }
        );
    }

    #[tokio::test]
    async fn drive_stream_collects_translation_text_and_plays_each_audio_frame() {
        use crate::application::fakes::FakeStream;

        let speech = FakeSpeech::default();
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = RealtimeUseCase::new(&speech, &audio, &codec);
        let o = opts(RealtimeMode::Translate);
        // A realistic server-side translate stream: a source transcript, its
        // target-language translation, two synthesized audio chunks, then Done.
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Transcript {
                text: "bonjour le monde".into(),
            },
            RealtimeFrame::Translation {
                text: "hello world".into(),
            },
            RealtimeFrame::Audio {
                data: b"PART1".to_vec(),
                format: Some("mp3".into()),
                seq: Some(0),
            },
            RealtimeFrame::Audio {
                data: b"PART2".to_vec(),
                format: Some("mp3".into()),
                seq: Some(1),
            },
            RealtimeFrame::Done,
        ]);

        let mut translations = Vec::new();
        let mut played = 0_usize;
        uc.drive_stream(&mut stream, &o, |e| match e {
            RealtimeEvent::Text {
                kind: FrameKind::Translation,
                text,
            } => translations.push(text.clone()),
            RealtimeEvent::Played => played += 1,
            _ => {}
        })
        .await
        .unwrap();

        assert_eq!(translations, vec!["hello world".to_owned()]);
        assert_eq!(played, 2, "each audio frame is decoded and played");
        assert_eq!(audio.plays.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn drive_stream_completes_when_the_stream_ends_without_a_done_frame() {
        use crate::application::fakes::FakeStream;

        let speech = FakeSpeech::default();
        let audio = FakeAudio::default();
        let codec = FakeCodec;
        let uc = RealtimeUseCase::new(&speech, &audio, &codec);
        let o = opts(RealtimeMode::Translate);
        // No terminal frame: natural exhaustion (recv -> None) must end the loop
        // without hanging or erroring (a dropped-but-not-failed stream).
        let mut stream = FakeStream::new(vec![
            RealtimeFrame::Transcript { text: "uno".into() },
            RealtimeFrame::Translation { text: "one".into() },
        ]);

        let mut events = Vec::new();
        uc.drive_stream(&mut stream, &o, |e| events.push(e.clone()))
            .await
            .unwrap();

        assert_eq!(
            events,
            vec![
                text_event(FrameKind::Transcript, "uno".into()),
                text_event(FrameKind::Translation, "one".into()),
            ]
        );
        assert!(
            audio.plays.lock().unwrap().is_empty(),
            "no audio frames to play"
        );
    }
}
