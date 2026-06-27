//! `voices` use case (T042): manage saved cloneable voices (FR-5).
//!
//! Drives the `VoiceRepository` **Repository** port (`add`/`list`/`remove`) — the
//! single application seam the CLI and daemon share for saved-voice management.
//! Reading the reference audio file is a driving-adapter concern; this use case
//! receives the bytes and orchestrates the port.

use anyhow::Result;

use crate::domain::voice::Voice;
use crate::ports::voice::VoiceRepository;

/// The `voices` use case over the [`VoiceRepository`] port.
pub struct VoicesUseCase<'a, R> {
    repository: &'a R,
}

impl<'a, R> VoicesUseCase<'a, R>
where
    R: VoiceRepository,
{
    /// Wire the use case to its port.
    #[must_use]
    pub fn new(repository: &'a R) -> Self {
        Self { repository }
    }

    /// Register a voice from reference `audio` with an optional `ref_text`.
    pub async fn add(&self, name: &str, audio: &[u8], ref_text: Option<&str>) -> Result<()> {
        self.repository.add(name, audio, ref_text).await
    }

    /// List the saved voices.
    pub async fn list(&self) -> Result<Vec<Voice>> {
        self.repository.list().await
    }

    /// Delete the saved voice named `name`.
    pub async fn remove(&self, name: &str) -> Result<()> {
        self.repository.remove(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeSpeech;

    #[tokio::test]
    async fn add_then_list_then_remove_round_trips() {
        let speech = FakeSpeech::default();
        let voices = VoicesUseCase::new(&speech);
        voices
            .add("narrator", b"\x00\x01", Some("the quick fox"))
            .await
            .unwrap();
        voices.add("robot", b"\x02", None).await.unwrap();

        let listed = voices.list().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .any(|v| v.name() == "narrator" && v.has_ref_text())
        );
        assert!(
            listed
                .iter()
                .any(|v| v.name() == "robot" && !v.has_ref_text())
        );

        voices.remove("narrator").await.unwrap();
        let after = voices.list().await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].name(), "robot");
    }
}
