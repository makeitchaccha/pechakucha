mod cache;
pub mod google_cloud;
pub mod registry;
pub mod voicevox;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;

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

    async fn generate(&self, text: &str) -> Result<AudioOutput, VoiceError>;
}

/// Audio produced by a voice. Stream chunks are consecutive bytes of one audio
/// container; chunk boundaries do not imply codec or frame boundaries.
pub enum AudioOutput {
    Buffered(Bytes),
    Stream {
        chunks: mpsc::Receiver<Result<Bytes, VoiceError>>,
        timing: StreamTimingProfile,
    },
}

/// Voice-specific measured stream timing data for playback policy.
#[derive(Debug)]
pub struct StreamTimingProfile {
    /// Carries the synthesis request's tracing context into Session logs.
    pub span: tracing::Span,
    /// Request start used to calculate elapsed time in the startup policy.
    pub request_started_at: tokio::time::Instant,
    /// Predicted total time from request start until the complete response.
    pub estimated_total_receive_time: std::time::Duration,
    /// Estimated playback duration for the complete requested utterance.
    pub total_audio_playback_duration: std::time::Duration,
    /// Whether segment receive-time variability has enough samples for an estimate.
    pub receive_uncertainty: SegmentReceiveUncertainty,
    /// User-selected number of segment-time standard deviations for buffering.
    pub buffer_sigma: f64,
    pub chunk_audio_duration: std::time::Duration,
}

#[derive(Clone, Copy, Debug)]
pub enum SegmentReceiveUncertainty {
    Warmup,
    Estimated { standard_deviation_secs: f64 },
}

impl AudioOutput {
    pub async fn into_bytes(self) -> Result<Bytes, VoiceError> {
        match self {
            Self::Buffered(bytes) => Ok(bytes),
            Self::Stream { mut chunks, .. } => {
                let mut output = Vec::new();
                while let Some(chunk) = chunks.recv().await {
                    output.extend_from_slice(&chunk?);
                }
                Ok(Bytes::from(output))
            }
        }
    }
}

#[cfg(test)]
pub mod test_utils {
    use crate::tts::{AudioOutput, Voice, VoiceError};
    use async_trait::async_trait;
    use bytes::Bytes;
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

        async fn generate(&self, text: &str) -> Result<AudioOutput, VoiceError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(AudioOutput::Buffered(Bytes::copy_from_slice(
                text.as_bytes(),
            )))
        }
    }
}
