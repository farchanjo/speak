//! Application layer: use cases that orchestrate the driven ports (ADR-0003).
//!
//! Each use case depends inward on the pure [`crate::domain`] value objects and
//! the [`crate::ports`] traits only — no `reqwest`/`ffmpeg`/`objc2`/`async-openai`
//! type crosses this boundary. The use cases are generic over the ports they
//! need (the composition root injects the concrete adapters, each optionally
//! wrapped in its retry decorator), so they are unit-testable with the in-memory
//! doubles in [`fakes`]. The application [`Facade`] exposes one cohesive surface
//! shared by the CLI and the daemon driving adapters (ADR-0005).

pub mod check;
pub mod playback;
pub mod realtime;
pub mod record;
pub mod say;
pub mod transcribe;
pub mod translate;
pub mod voices;

pub use check::{CheckOutcome, CheckUseCase, HealthOutcome};
pub use playback::PlaybackStats;
pub use realtime::{FrameKind, RealtimeEvent, RealtimeOptions, RealtimeStep, RealtimeUseCase};
pub use record::{RecordOptions, RecordOutcome, RecordUseCase};
pub use say::{SayOptions, SayOutcome, SayUseCase};
pub use transcribe::TranscribeUseCase;
pub use translate::TranslateUseCase;
pub use voices::VoicesUseCase;

#[cfg(test)]
pub(crate) mod fakes;
