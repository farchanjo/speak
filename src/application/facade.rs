//! Application **Facade** (T045): one cohesive surface over the use cases.
//!
//! Both driving adapters — the CLI (`src/main.rs`) and the daemon
//! (`adapters/daemon`, ADR-0005) — call this single object so a request takes the
//! identical path whether it runs in-process or is forwarded over the socket. The
//! Facade is generic over the three adapter roles the composition root injects
//! (`Speech` = the openai adapter, `Audio` = coreaudio, `Codec` = libav, each
//! optionally wrapped in its retry decorator) and owns them; every method builds
//! the relevant use case from borrows and delegates. Per-method `where` bounds
//! keep the Facade constructible even when a role does not satisfy every port, so
//! the surface degrades feature-by-feature rather than all-or-nothing.

use anyhow::Result;

use crate::adapters::libav::accel::Report as AccelReport;
use crate::application::check::{CheckOutcome, CheckUseCase, HealthOutcome};
use crate::application::realtime::{RealtimeEvent, RealtimeOptions, RealtimeStep, RealtimeUseCase};
use crate::application::record::{RecordOptions, RecordOutcome, RecordUseCase};
use crate::application::say::{SayOptions, SayOutcome, SayUseCase};
use crate::application::stream_transcribe::{
    StreamTranscribeOptions, StreamTranscribeUseCase, TranscribeStreamEnd,
};
use crate::application::transcribe::TranscribeUseCase;
use crate::application::translate::TranslateUseCase;
use crate::application::voices::VoicesUseCase;
use crate::domain::language::Language;
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::Voice;
use crate::ports::audio::{AudioSink, AudioSource};
use crate::ports::codec::{AudioDecoder, AudioEncoder};
use crate::ports::probe::ServerProbe;
use crate::ports::realtime::{RealtimeFrame, RealtimeStream};
use crate::ports::synthesizer::Synthesizer;
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;
use crate::ports::voice::VoiceRepository;

/// The application Facade over the speech, audio, and codec adapter roles.
pub struct SpeakFacade<Speech, Audio, Codec> {
    speech: Speech,
    audio: Audio,
    codec: Codec,
}

impl<Speech, Audio, Codec> SpeakFacade<Speech, Audio, Codec> {
    /// Build the Facade from the three injected adapters (Factory step).
    #[must_use]
    pub fn new(speech: Speech, audio: Audio, codec: Codec) -> Self {
        Self {
            speech,
            audio,
            codec,
        }
    }

    /// Synthesize `spec` and optionally play it (FR-1 / FR-11).
    pub async fn say(&self, spec: &SpeechSpec, opts: &SayOptions) -> Result<SayOutcome>
    where
        Speech: Synthesizer,
        Codec: AudioDecoder,
        Audio: AudioSink,
    {
        SayUseCase::new(&self.speech, &self.codec, &self.audio)
            .execute(spec, opts)
            .await
    }

    /// Transcribe an uploaded audio file (FR-6).
    pub async fn transcribe(&self, req: &TranscribeRequest<'_>) -> Result<String>
    where
        Speech: Transcriber,
    {
        TranscribeUseCase::new(&self.speech).execute(req).await
    }

    /// Translate an uploaded audio file into `target`-language text (FR-7).
    pub async fn translate(&self, audio: &[u8], filename: &str, target: &Language) -> Result<String>
    where
        Speech: Translator,
    {
        TranslateUseCase::new(&self.speech)
            .execute(audio, filename, target)
            .await
    }

    /// Register a saved voice from reference audio (FR-5).
    pub async fn add_voice(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()>
    where
        Speech: VoiceRepository,
    {
        VoicesUseCase::new(&self.speech)
            .add(name, audio, ref_text)
            .await
    }

    /// List the saved voices (FR-5).
    pub async fn list_voices(&self) -> Result<Vec<Voice>>
    where
        Speech: VoiceRepository,
    {
        VoicesUseCase::new(&self.speech).list().await
    }

    /// Delete a saved voice by name (FR-5).
    pub async fn remove_voice(&self, name: &str) -> Result<()>
    where
        Speech: VoiceRepository,
    {
        VoicesUseCase::new(&self.speech).remove(name).await
    }

    /// Capture the microphone to WAV/FLAC bytes (FR-9).
    pub async fn record(&self, opts: &RecordOptions) -> Result<RecordOutcome>
    where
        Audio: AudioSource,
        Codec: AudioDecoder + AudioEncoder,
    {
        RecordUseCase::new(&self.audio, &self.codec, &self.codec)
            .execute(opts)
            .await
    }

    /// Process one realtime capture chunk (FR-8).
    pub async fn realtime_step(&self, opts: &RealtimeOptions) -> Result<Option<RealtimeStep>>
    where
        Speech: Synthesizer + Transcriber + Translator,
        Audio: AudioSource + AudioSink,
        Codec: AudioDecoder + AudioEncoder,
    {
        self.realtime().step(opts).await
    }

    /// Process one realtime SSE frame (FR-8).
    pub async fn realtime_frame(
        &self,
        frame: RealtimeFrame,
        opts: &RealtimeOptions,
    ) -> Result<RealtimeEvent>
    where
        Speech: Synthesizer + Transcriber + Translator,
        Audio: AudioSource + AudioSink,
        Codec: AudioDecoder + AudioEncoder,
    {
        self.realtime().pump_frame(frame, opts).await
    }

    /// Capture one realtime chunk encoded as WAV for the SSE endpoint (FR-8, T036).
    pub async fn realtime_capture(&self, opts: &RealtimeOptions) -> Result<Option<Vec<u8>>>
    where
        Speech: Synthesizer + Transcriber + Translator,
        Audio: AudioSource + AudioSink,
        Codec: AudioDecoder + AudioEncoder,
    {
        self.realtime().capture_chunk(opts).await
    }

    /// Drive a realtime SSE stream to completion, invoking `on_event` per frame.
    pub async fn realtime_drive<St, F>(
        &self,
        stream: &mut St,
        opts: &RealtimeOptions,
        on_event: F,
    ) -> Result<()>
    where
        St: RealtimeStream,
        F: FnMut(&RealtimeEvent),
        Speech: Synthesizer + Transcriber + Translator,
        Audio: AudioSource + AudioSink,
        Codec: AudioDecoder + AudioEncoder,
    {
        self.realtime().drive_stream(stream, opts, on_event).await
    }

    /// Capture one streaming-transcribe chunk as WAV, gated by VAD (ADR-0014).
    pub async fn stream_transcribe_capture(
        &self,
        opts: &StreamTranscribeOptions,
    ) -> Result<Option<Vec<u8>>>
    where
        Audio: AudioSource,
        Codec: AudioDecoder + AudioEncoder,
    {
        StreamTranscribeUseCase::new(&self.audio, &self.codec)
            .capture(opts)
            .await
    }

    /// Drive a transcript-only SSE stream, invoking `on_transcript` per frame
    /// and ignoring re-voiced audio/translation frames (ADR-0014).
    pub async fn stream_transcribe_drive<St, F>(
        &self,
        stream: &mut St,
        on_transcript: F,
    ) -> Result<TranscribeStreamEnd>
    where
        St: RealtimeStream,
        F: FnMut(&str),
        Audio: AudioSource,
        Codec: AudioDecoder + AudioEncoder,
    {
        StreamTranscribeUseCase::new(&self.audio, &self.codec)
            .drive(stream, on_transcript)
            .await
    }

    /// Whether the server advertises the realtime SSE endpoint (FR-14, ADR-0004).
    pub async fn supports_realtime(&self) -> Result<bool>
    where
        Speech: ServerProbe,
    {
        self.speech.supports_realtime().await
    }

    /// Lightweight upstream liveness probe (`GET /health`) for the daemon health
    /// watchdog (ADR-0010): a single request, distinct from the richer
    /// [`health`](Self::health) report that also reads models + capability.
    pub async fn probe_health(&self) -> Result<bool>
    where
        Speech: ServerProbe,
    {
        self.speech.health().await
    }

    /// Probe server health, models, and realtime capability (FR-14).
    pub async fn health(&self) -> Result<HealthOutcome>
    where
        Speech: ServerProbe,
    {
        CheckUseCase::new(&self.speech).health().await
    }

    /// Build the full `check` diagnostic (server health + local accel, FR-14).
    pub async fn check(&self, host: &str, accel: AccelReport) -> Result<CheckOutcome>
    where
        Speech: ServerProbe,
    {
        CheckUseCase::new(&self.speech).check(host, accel).await
    }

    /// The realtime use case wired to this Facade's adapter roles.
    fn realtime(&self) -> RealtimeUseCase<'_, Speech, Audio, Codec>
    where
        Speech: Synthesizer + Transcriber + Translator,
        Audio: AudioSource + AudioSink,
        Codec: AudioDecoder + AudioEncoder,
    {
        RealtimeUseCase::new(&self.speech, &self.audio, &self.codec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec, FakeSpeech};
    use crate::domain::gen_params::GenParams;
    use crate::domain::realtime::RealtimeMode;
    use crate::domain::voice::{StandardVoice, VoiceMode};

    fn facade() -> SpeakFacade<FakeSpeech, FakeAudio, FakeCodec> {
        SpeakFacade::new(FakeSpeech::default(), FakeAudio::default(), FakeCodec)
    }

    fn spec() -> SpeechSpec {
        SpeechSpec::builder("hi there")
            .voice(VoiceMode::Standard(StandardVoice::new("alloy").unwrap()))
            .language(Language::parse("en").unwrap())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn say_runs_through_the_facade() {
        let out = facade().say(&spec(), &SayOptions::default()).await.unwrap();
        assert!(out.playback.is_some());
        assert_eq!(out.audio.audio_seconds.as_deref(), Some("1.0"));
    }

    #[tokio::test]
    async fn transcribe_and_translate_share_the_facade() {
        let f = facade();
        let lang = Language::parse("en").unwrap();
        let req = TranscribeRequest {
            audio: b"\x00",
            filename: "a.wav",
            language: None,
            format: "json",
        };
        assert_eq!(f.transcribe(&req).await.unwrap(), "hello");
        assert_eq!(f.translate(b"\x00", "a.wav", &lang).await.unwrap(), "olá");
    }

    #[tokio::test]
    async fn voices_round_trip_through_the_facade() {
        let f = facade();
        f.add_voice("narrator", b"\x00", Some("ref")).await.unwrap();
        assert_eq!(f.list_voices().await.unwrap().len(), 1);
        f.remove_voice("narrator").await.unwrap();
        assert!(f.list_voices().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn record_through_the_facade_returns_wav_bytes() {
        let opts = RecordOptions {
            source: crate::domain::capture_source::CaptureSource::input(None, None),
            secs: 1.0,
            format: crate::ports::codec::RecordFormat::Wav,
            sample_rate: None,
            channels: None,
        };
        let out = facade().record(&opts).await.unwrap();
        assert_eq!(&out.bytes[0..4], b"RIFF");
    }

    #[tokio::test]
    async fn realtime_and_health_through_the_facade() {
        let f = facade();
        let r_opts = RealtimeOptions {
            mode: RealtimeMode::Translate,
            from: None,
            to: Language::parse("en").unwrap(),
            voice: VoiceMode::Standard(StandardVoice::new("alloy").unwrap()),
            output_language: Language::parse("en").unwrap(),
            format: crate::domain::audio_format::AudioFormat::Mp3,
            speed: 1.0,
            gen_params: GenParams::new(),
            chunk_secs: 5.0,
            source: crate::domain::capture_source::CaptureSource::input(None, None),
            outputs: Vec::new(),
            volume: 1.0,
            vad: false,
            silence_floor: 0.1,
        };
        assert!(f.realtime_step(&r_opts).await.unwrap().is_some());
        let frame = RealtimeFrame::Done;
        assert_eq!(
            f.realtime_frame(frame, &r_opts).await.unwrap(),
            RealtimeEvent::Done
        );
        assert!(f.health().await.unwrap().healthy);
    }
}
