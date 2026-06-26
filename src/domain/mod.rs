//! Pure domain layer: value objects with zero IO (ADR-0003).
//!
//! Nothing here performs network, filesystem, or audio IO. The driving and
//! driven adapters depend inward on these types; the dependency never points
//! the other way. The value objects model the synthesis request
//! ([`speech_spec::SpeechSpec`] over [`voice::VoiceMode`], [`language::Language`],
//! [`audio_format::AudioFormat`], [`gen_params`]), the audio data
//! ([`pcm::PcmBuffer`] / [`pcm::SampleFormat`]), the realtime strategy
//! ([`realtime::RealtimeMode`]), the resilience policy ([`retry::RetryPolicy`]),
//! and the shared failure vocabulary ([`errors::DomainError`]).

pub mod audio_format;
pub mod errors;
pub mod gen_params;
pub mod language;
pub mod pcm;
pub mod realtime;
pub mod retry;
pub mod speech_spec;
pub mod voice;
pub mod voice_design;
