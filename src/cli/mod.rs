//! `cli` driving adapter (T050-T056): the clap surface that maps arguments to
//! application use cases and contains no business logic (ADR-0003).
//!
//! The composition root (`src/main.rs`) builds the concrete [`AppFacade`] object
//! graph (Factory/DI) and dispatches each parsed subcommand to the handler here.
//! Every handler maps CLI arguments to domain value objects, calls the
//! [`speak::application::SpeakFacade`], and renders the result. File I/O (reading
//! input audio, writing `-o`/record output) stays in this driving adapter because
//! the application layer is framework-free and returns owned bytes to persist.

use std::path::Path;

use speak::adapters::coreaudio::CoreAudio;
use speak::adapters::libav::LibavCodec;
use speak::adapters::openai::OpenAiAdapter;
use speak::application::SpeakFacade;

pub mod args;
pub mod check;
pub mod completions;
pub mod config;
pub mod devices;
pub mod realtime;
pub mod say;
pub mod transcribe;
pub mod translate;
pub mod voices;

/// The concrete application Facade the composition root injects into every
/// handler: the `openai` speech adapter, the `coreaudio` audio adapter, and the
/// `libav` codec adapter wired together (ADR-0003 / T054).
pub type AppFacade = SpeakFacade<OpenAiAdapter, CoreAudio, LibavCodec>;

/// Extract a multipart-friendly basename from `path`, with a stable fallback.
#[must_use]
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_extracts_basename_with_fallback() {
        assert_eq!(file_name(Path::new("/a/b/clip.wav")), "clip.wav");
        assert_eq!(file_name(Path::new("plain.mp3")), "plain.mp3");
        assert_eq!(file_name(Path::new("/")), "audio");
    }
}
