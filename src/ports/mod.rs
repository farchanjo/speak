//! Driven-port traits (ADR-0003).
//!
//! The narrow interfaces the application use cases depend on; the `adapters/*`
//! modules implement them and the composition root injects the concrete
//! implementations (wrapped in their retry decorators). Dependencies point
//! inward: ports reference only the pure [`crate::domain`] value objects (plus
//! the still-flat resolved [`crate::config::Config`] POD, which moves inward
//! with the config adapter in a later stage). No `reqwest`/`ffmpeg`/`objc2`/
//! `async-openai` type appears in any port signature.

pub mod audio;
pub mod codec;
pub mod config;
pub mod probe;
pub mod realtime;
pub mod retry;
pub mod synthesizer;
pub mod transcriber;
pub mod translator;
pub mod voice;

pub use audio::{AudioDevice, AudioDeviceId, AudioSink, AudioSource};
pub use codec::{AudioDecoder, AudioEncoder, RecordFormat};
pub use config::ConfigProvider;
pub use probe::ServerProbe;
pub use realtime::{RealtimeFrame, RealtimeStream};
pub use retry::RetryPolicy;
pub use synthesizer::{SynthesizedAudio, Synthesizer};
pub use transcriber::{TranscribeRequest, Transcriber};
pub use translator::Translator;
pub use voice::VoiceRepository;
