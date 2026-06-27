//! macOS `CoreAudio` / `AVFAudio` backend for the `coreaudio` adapter.
//!
//! Splits into device enumeration ([`device`]) and the `AVAudioEngine` playback /
//! capture graph ([`engine`]); the parent module wires both behind the
//! `AudioSink` / `AudioSource` ports.

mod device;
mod disclaim;
mod engine;
mod stream;
mod tap;

pub use device::enumerate;
pub use disclaim::reexec_disclaimed;
pub use engine::{capture, play, play_to};
pub(crate) use stream::start_capture_stream;
pub use tap::capture_output;
