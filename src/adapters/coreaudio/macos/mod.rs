//! macOS `CoreAudio` / `AVFAudio` backend for the `coreaudio` adapter.
//!
//! Splits into device enumeration ([`device`]) and the `AVAudioEngine` playback /
//! capture graph ([`engine`]); the parent module wires both behind the
//! `AudioSink` / `AudioSource` ports.

mod device;
mod engine;

pub use device::enumerate;
pub use engine::{capture, play, play_to};
