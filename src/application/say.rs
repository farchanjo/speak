//! `say` use case (T040): synthesize speech, then optionally play it.
//!
//! Orchestrates the `Synthesizer` -> `AudioDecoder` -> `AudioSink` ports (FR-1 /
//! FR-11): synthesize the validated [`SpeechSpec`], and — unless `--no-play` —
//! decode the bytes and route them to the default device or fan them out to
//! several `--output-device`s. The encoded bytes and the server timing headers
//! are returned so the driving adapter can honour `-o`/`--json`. No framework
//! type crosses the boundary.

use anyhow::Result;

use crate::application::playback::{self, PlaybackStats};
use crate::domain::speech_spec::SpeechSpec;
use crate::ports::audio::{AudioDeviceId, AudioSink};
use crate::ports::codec::AudioDecoder;
use crate::ports::synthesizer::{SynthesizedAudio, Synthesizer};

/// Playback options for a `say` invocation.
#[derive(Debug, Clone)]
pub struct SayOptions {
    /// Whether to play the synthesized audio locally.
    pub play: bool,
    /// Mixer volume in `0.0..=1.0`.
    pub volume: f32,
    /// Target output devices; empty = system default (fan-out when > 1, FR-11).
    pub devices: Vec<AudioDeviceId>,
}

impl Default for SayOptions {
    fn default() -> Self {
        Self {
            play: true,
            volume: 1.0,
            devices: Vec::new(),
        }
    }
}

/// The result of a `say` invocation.
#[derive(Debug, Clone)]
pub struct SayOutcome {
    /// Synthesized bytes + the server's `X-RTF`/`X-Audio-Seconds` (FR-1).
    pub audio: SynthesizedAudio,
    /// Decoded playback statistics when played, else `None`.
    pub playback: Option<PlaybackStats>,
}

/// The `say` use case over the synthesis, codec, and sink ports.
pub struct SayUseCase<'a, S, D, K> {
    synthesizer: &'a S,
    decoder: &'a D,
    sink: &'a K,
}

impl<'a, S, D, K> SayUseCase<'a, S, D, K>
where
    S: Synthesizer,
    D: AudioDecoder,
    K: AudioSink,
{
    /// Wire the use case to its ports.
    #[must_use]
    pub fn new(synthesizer: &'a S, decoder: &'a D, sink: &'a K) -> Self {
        Self {
            synthesizer,
            decoder,
            sink,
        }
    }

    /// Synthesize `spec` and play it according to `opts`.
    pub async fn execute(&self, spec: &SpeechSpec, opts: &SayOptions) -> Result<SayOutcome> {
        let audio = self.synthesizer.synthesize(spec).await?;
        let playback = if opts.play {
            let stats = playback::decode_and_play(
                self.decoder,
                self.sink,
                &audio.bytes,
                &opts.devices,
                opts.volume,
            )
            .await?;
            Some(stats)
        } else {
            None
        };
        Ok(SayOutcome { audio, playback })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAudio, FakeCodec, FakeSpeech};
    use crate::domain::language::Language;
    use crate::domain::voice::{StandardVoice, VoiceMode};

    fn spec() -> SpeechSpec {
        SpeechSpec::builder("hello there")
            .voice(VoiceMode::Standard(StandardVoice::new("alloy").unwrap()))
            .language(Language::parse("en").unwrap())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn plays_on_default_device_and_returns_metadata() {
        let speech = FakeSpeech::default();
        let codec = FakeCodec;
        let audio = FakeAudio::default();
        let outcome = SayUseCase::new(&speech, &codec, &audio)
            .execute(&spec(), &SayOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.audio.rtf.as_deref(), Some("0.42"));
        let plays = audio.plays.lock().unwrap();
        assert_eq!(plays.len(), 1);
        assert!(plays[0].devices.is_empty(), "default device => no fan-out");
        assert!(outcome.playback.is_some());
    }

    #[tokio::test]
    async fn no_play_skips_the_sink_but_still_synthesizes() {
        let speech = FakeSpeech::default();
        let codec = FakeCodec;
        let audio = FakeAudio::default();
        let opts = SayOptions {
            play: false,
            ..SayOptions::default()
        };
        let outcome = SayUseCase::new(&speech, &codec, &audio)
            .execute(&spec(), &opts)
            .await
            .unwrap();
        assert!(audio.plays.lock().unwrap().is_empty());
        assert!(outcome.playback.is_none());
        assert_eq!(speech.synth_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn multiple_devices_fan_out_to_every_target() {
        let speech = FakeSpeech::default();
        let codec = FakeCodec;
        let audio = FakeAudio::default();
        let opts = SayOptions {
            devices: vec![AudioDeviceId(3), AudioDeviceId(7)],
            volume: 0.5,
            ..SayOptions::default()
        };
        SayUseCase::new(&speech, &codec, &audio)
            .execute(&spec(), &opts)
            .await
            .unwrap();
        let plays = audio.plays.lock().unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].devices, vec![AudioDeviceId(3), AudioDeviceId(7)]);
        assert!((plays[0].volume - 0.5).abs() < f32::EPSILON);
    }
}
