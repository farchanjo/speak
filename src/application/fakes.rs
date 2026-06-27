//! In-memory port doubles shared by the application use-case unit tests.
//!
//! The use cases are generic over the driven ports, so they can be exercised
//! with no network or audio hardware by injecting these fakes. They are grouped
//! by adapter role to mirror the real composition (`openai` -> [`FakeSpeech`],
//! `coreaudio` -> [`FakeAudio`], `libav` -> [`FakeCodec`]), which also lets the
//! `Facade` tests reuse the exact same doubles.

#![cfg(test)]

use std::sync::Mutex;

use anyhow::Result;

use crate::domain::language::Language;
use crate::domain::pcm::PcmBuffer;
use crate::domain::speech_spec::SpeechSpec;
use crate::domain::voice::Voice;
use crate::ports::audio::{AudioDevice, AudioDeviceId, AudioSink, AudioSource};
use crate::ports::codec::{AudioDecoder, AudioEncoder, RecordFormat};
use crate::ports::probe::ServerProbe;
use crate::ports::synthesizer::{SynthesizedAudio, Synthesizer};
use crate::ports::transcriber::{TranscribeRequest, Transcriber};
use crate::ports::translator::Translator;
use crate::ports::voice::VoiceRepository;

/// Server-facing role: synthesize, transcribe, translate, voices, and probe.
pub(crate) struct FakeSpeech {
    /// Bytes returned by every `synthesize` call.
    pub audio_bytes: Vec<u8>,
    /// Text returned by every `transcribe` call.
    pub transcript: String,
    /// Text returned by every `translate` call.
    pub translation: String,
    /// `health` result.
    pub healthy: bool,
    /// `models` result.
    pub models: Vec<String>,
    /// `supports_realtime` result.
    pub realtime: bool,
    /// Saved voices backing the repository.
    pub voices: Mutex<Vec<Voice>>,
    /// Every spec passed to `synthesize`, for assertions.
    pub synth_calls: Mutex<Vec<SpeechSpec>>,
}

impl Default for FakeSpeech {
    fn default() -> Self {
        Self {
            audio_bytes: b"AUDIO".to_vec(),
            transcript: "hello".to_owned(),
            translation: "olá".to_owned(),
            healthy: true,
            models: vec!["tts-1".to_owned(), "whisper-1".to_owned()],
            realtime: true,
            voices: Mutex::new(Vec::new()),
            synth_calls: Mutex::new(Vec::new()),
        }
    }
}

impl Synthesizer for FakeSpeech {
    async fn synthesize(&self, spec: &SpeechSpec) -> Result<SynthesizedAudio> {
        self.synth_calls.lock().unwrap().push(spec.clone());
        Ok(SynthesizedAudio {
            bytes: self.audio_bytes.clone(),
            content_type: "audio/mpeg".to_owned(),
            rtf: Some("0.42".to_owned()),
            audio_seconds: Some("1.0".to_owned()),
        })
    }
}

impl Transcriber for FakeSpeech {
    async fn transcribe(&self, _req: &TranscribeRequest<'_>) -> Result<String> {
        Ok(self.transcript.clone())
    }
}

impl Translator for FakeSpeech {
    async fn translate(
        &self,
        _audio: &[u8],
        _filename: &str,
        _target: &Language,
    ) -> Result<String> {
        Ok(self.translation.clone())
    }
}

impl VoiceRepository for FakeSpeech {
    async fn add(&self, name: &str, _audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        self.voices
            .lock()
            .unwrap()
            .push(Voice::new(name, ref_text.is_some())?);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Voice>> {
        Ok(self.voices.lock().unwrap().clone())
    }

    async fn remove(&self, name: &str) -> Result<()> {
        self.voices.lock().unwrap().retain(|v| v.name() != name);
        Ok(())
    }
}

impl ServerProbe for FakeSpeech {
    async fn health(&self) -> Result<bool> {
        Ok(self.healthy)
    }

    async fn models(&self) -> Result<Vec<String>> {
        Ok(self.models.clone())
    }

    async fn supports_realtime(&self) -> Result<bool> {
        Ok(self.realtime)
    }
}

/// Codec role: decode/resample and encode record containers.
#[derive(Default)]
pub(crate) struct FakeCodec;

impl AudioDecoder for FakeCodec {
    fn decode(&self, bytes: &[u8]) -> Result<PcmBuffer> {
        // One mono 48 kHz sample per input byte so playback stats are non-empty.
        Ok(PcmBuffer::new(vec![0.0; bytes.len().max(1)], 48_000, 1))
    }

    fn resample(&self, pcm: &PcmBuffer, sample_rate: u32, channels: u16) -> Result<PcmBuffer> {
        // Preserve the input's signal level so the realtime silence gate, which
        // runs on the resampled mono buffer, can tell silence from speech.
        let frames = pcm.frames().max(1);
        let level = pcm.samples().first().copied().unwrap_or(0.0);
        Ok(PcmBuffer::new(
            vec![level; frames * usize::from(channels.max(1))],
            sample_rate,
            channels,
        ))
    }
}

impl AudioEncoder for FakeCodec {
    fn encode(&self, pcm: &PcmBuffer, format: RecordFormat) -> Result<Vec<u8>> {
        let mut out = match format {
            RecordFormat::Wav => b"RIFF".to_vec(),
            RecordFormat::Flac => b"fLaC".to_vec(),
        };
        out.extend((pcm.frames() as u32).to_le_bytes());
        Ok(out)
    }
}

/// A single recorded `play` / `play_to` invocation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlayCall {
    /// Frames in the buffer that was played.
    pub frames: usize,
    /// Target devices (empty = default device).
    pub devices: Vec<AudioDeviceId>,
    /// Requested mixer volume.
    pub volume: f32,
}

/// Native-audio role: capture from the mic and play to one or many devices.
pub(crate) struct FakeAudio {
    /// Buffer returned by every `capture` call.
    pub capture_pcm: PcmBuffer,
    /// Recorded playback invocations.
    pub plays: Mutex<Vec<PlayCall>>,
    /// Output devices enumerated.
    pub outputs: Vec<AudioDevice>,
    /// Input devices enumerated.
    pub inputs: Vec<AudioDevice>,
}

impl Default for FakeAudio {
    fn default() -> Self {
        Self {
            capture_pcm: PcmBuffer::new(vec![0.5; 96_000], 48_000, 2),
            plays: Mutex::new(Vec::new()),
            outputs: Vec::new(),
            inputs: Vec::new(),
        }
    }
}

impl AudioSink for FakeAudio {
    async fn play(&self, pcm: &PcmBuffer, volume: f32) -> Result<()> {
        self.plays.lock().unwrap().push(PlayCall {
            frames: pcm.frames(),
            devices: Vec::new(),
            volume,
        });
        Ok(())
    }

    async fn play_to(&self, pcm: &PcmBuffer, devices: &[AudioDeviceId], volume: f32) -> Result<()> {
        self.plays.lock().unwrap().push(PlayCall {
            frames: pcm.frames(),
            devices: devices.to_vec(),
            volume,
        });
        Ok(())
    }

    fn outputs(&self) -> Result<Vec<AudioDevice>> {
        Ok(self.outputs.clone())
    }
}

impl AudioSource for FakeAudio {
    async fn capture(&self, _device: Option<AudioDeviceId>, _secs: f64) -> Result<PcmBuffer> {
        Ok(self.capture_pcm.clone())
    }

    fn inputs(&self) -> Result<Vec<AudioDevice>> {
        Ok(self.inputs.clone())
    }
}
