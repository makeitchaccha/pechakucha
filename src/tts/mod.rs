mod cache;
pub mod google_cloud;
pub mod registry;

use async_trait::async_trait;

use thiserror::Error;

const DISCORD_SAMPLE_RATE: i32 = 48_000;

#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("API request failed: {0}")]
    Api(anyhow::Error),
    #[error("Cache error: {0}")]
    Cache(anyhow::Error),
    #[error("Unknown error: {0}")]
    Unknown(anyhow::Error),
}

pub struct VoiceDetail {
    pub name: String,
    pub provider: String,
    pub description: Option<String>,
}

/// # Voice: normalized pcm ready to play with songbird
///
/// Contrary with Source, Voice is a normalized audio
/// trans-coded from source result.
#[async_trait]
pub trait Voice: Send + Sync {
    fn identifier(&self) -> &str;

    /// Returns the language code associated with this voice.
    ///
    /// The language code should be in ISO 639-1 format (e.g., "en", "ja") or BCP 47 format (e.g., "en-US", "ja-JP"),
    /// depending on the requirements of the localization system.
    fn language(&self) -> &str;

    async fn generate(&self, text: &str) -> Result<Vec<u8>, VoiceError>;
}

#[cfg(test)]
pub mod test_utils {
    use crate::tts::{Voice, VoiceError};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    pub struct MockVoice {
        call_count: Arc<AtomicUsize>,
    }

    impl MockVoice {
        pub fn new() -> Self {
            Self {
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl Default for MockVoice {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait]
    impl Voice for MockVoice {
        fn identifier(&self) -> &str {
            "mock"
        }

        fn language(&self) -> &str {
            "mock-language"
        }

        async fn generate(&self, text: &str) -> Result<Vec<u8>, VoiceError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(text.as_bytes().to_vec())
        }
    }
}
