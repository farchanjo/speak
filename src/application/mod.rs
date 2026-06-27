//! Application layer: use cases that orchestrate the driven ports (ADR-0003).
//!
//! Each use case depends inward on the pure [`crate::domain`] value objects and
//! the [`crate::ports`] traits only — no `reqwest`/`ffmpeg`/`objc2`/`async-openai`
//! type crosses this boundary. The use cases are generic over the ports they
//! need (the composition root injects the concrete adapters, each optionally
//! wrapped in its retry decorator), so they are unit-testable with the in-memory
//! doubles in [`fakes`]. The application [`Facade`] exposes one cohesive surface
//! shared by the CLI and the daemon driving adapters (ADR-0005).

pub mod capture;
pub mod check;
pub mod facade;
pub mod playback;
pub mod realtime;
pub mod record;
pub mod say;
pub mod stream_transcribe;
pub mod transcribe;
pub mod translate;
pub mod voices;

pub use check::{CheckOutcome, CheckUseCase, HealthOutcome};
pub use facade::SpeakFacade;
pub use playback::PlaybackStats;
pub use realtime::{FrameKind, RealtimeEvent, RealtimeOptions, RealtimeStep, RealtimeUseCase};
pub use record::{RecordOptions, RecordOutcome, RecordUseCase};
pub use say::{SayOptions, SayOutcome, SayUseCase};
pub use stream_transcribe::{
    StreamTranscribeOptions, StreamTranscribeUseCase, TranscribeStreamEnd,
};
pub use transcribe::TranscribeUseCase;
pub use translate::TranslateUseCase;
pub use voices::VoicesUseCase;

/// Select one input channel from a multi-channel capture, or keep it whole.
///
/// `None` keeps the full buffer (the default mono-downmix path). `Some(ch)`
/// extracts that single 0-based channel so a mic on one input of a many-channel
/// interface (e.g. SSL 12) is not attenuated by averaging every channel into the
/// ASR/record downmix (ADR-0013). Errors when `ch` is out of range.
pub(crate) fn pick_input_channel(
    pcm: crate::domain::pcm::PcmBuffer,
    channel: Option<u16>,
) -> anyhow::Result<crate::domain::pcm::PcmBuffer> {
    let Some(ch) = channel else { return Ok(pcm) };
    let channels = pcm.channels();
    pcm.select_channel(ch).ok_or_else(|| {
        anyhow::anyhow!("input channel {ch} is out of range (device exposes {channels} channels)")
    })
}

#[cfg(test)]
pub(crate) mod fakes;
