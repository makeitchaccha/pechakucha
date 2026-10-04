mod cache;
pub mod google_cloud;
pub mod registry;
pub mod voicevox;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;

use thiserror::Error;

const DISCORD_SAMPLE_RATE_HZ: i32 = 48_000;
/// Conservative receive-time margin before segment variability is estimated.
const WARMUP_RECEIVE_MARGIN: std::time::Duration = std::time::Duration::from_secs(3);

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

/// Audio layout and timing information used by the streaming playback policy.
#[derive(Debug)]
pub struct StreamTimingProfile {
    /// Request start used to calculate elapsed time in the startup policy.
    pub request_started_at: tokio::time::Instant,
    /// Predicted total time from request start until the complete response.
    pub estimated_total_receive_time: std::time::Duration,
    /// Estimated playback duration for the complete requested utterance.
    pub total_audio_playback_duration: std::time::Duration,
    pub audio_format: StreamAudioFormat,
    /// Whether segment receive-time variability has enough samples for an estimate.
    pub receive_uncertainty: SegmentReceiveUncertainty,
    /// User-selected number of segment-time standard deviations for buffering.
    pub buffer_sigma: f64,
    pub chunk_audio_duration: std::time::Duration,
}

impl StreamTimingProfile {
    /// Returns the predicted delay until receive completion, including the
    /// conservative margin for the stream progress observed so far.
    pub(crate) fn predicted_receive_completion_delay(
        &self,
        now: tokio::time::Instant,
        received_container_bytes: u64,
    ) -> f64 {
        let elapsed = now.saturating_duration_since(self.request_started_at);
        let remaining_receive_secs = self
            .estimated_total_receive_time
            .saturating_sub(elapsed)
            .as_secs_f64();
        let received_audio_secs = self.received_audio_duration_secs(received_container_bytes);
        let remaining_audio_secs =
            (self.total_audio_playback_duration.as_secs_f64() - received_audio_secs).max(0.0);
        let segment_secs = self.chunk_audio_duration.as_secs_f64();
        let remaining_segments = if segment_secs <= 0.0 {
            0.0
        } else {
            (remaining_audio_secs / segment_secs).ceil()
        };
        let receive_margin_secs = match self.receive_uncertainty {
            SegmentReceiveUncertainty::Warmup => WARMUP_RECEIVE_MARGIN.as_secs_f64(),
            SegmentReceiveUncertainty::Estimated {
                standard_deviation_secs,
            } => self.buffer_sigma * standard_deviation_secs * remaining_segments.sqrt(),
        };

        remaining_receive_secs + receive_margin_secs
    }

    pub(crate) fn has_received_audio_payload(&self, received_container_bytes: u64) -> bool {
        self.audio_format
            .payload_bytes_received(received_container_bytes)
            > 0
    }

    fn received_audio_duration_secs(&self, received_container_bytes: u64) -> f64 {
        let received_pcm_bytes = self
            .audio_format
            .payload_bytes_received(received_container_bytes);
        self.audio_format
            .audio_seconds_for_payload_bytes(received_pcm_bytes)
    }

    /// Returns the playback duration available after reserving one chunk for startup.
    pub(crate) fn safe_playback_window(&self) -> std::time::Duration {
        self.total_audio_playback_duration
            .saturating_sub(self.chunk_audio_duration)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StreamAudioFormat {
    /// Container bytes that precede the PCM payload.
    pub container_header_bytes: u64,
    /// PCM payload bytes corresponding to one second of audio.
    pub pcm_payload_bytes_per_second: u64,
}

impl StreamAudioFormat {
    pub(crate) fn payload_bytes_received(self, container_bytes_received: u64) -> u64 {
        container_bytes_received.saturating_sub(self.container_header_bytes)
    }

    pub(crate) fn payload_bytes_in_chunk(
        self,
        container_bytes_received: u64,
        chunk_bytes: u64,
    ) -> u64 {
        let remaining_header_bytes = self
            .container_header_bytes
            .saturating_sub(container_bytes_received);
        chunk_bytes.saturating_sub(remaining_header_bytes)
    }

    pub(crate) fn audio_seconds_for_payload_bytes(self, payload_bytes: u64) -> f64 {
        payload_bytes as f64 / self.pcm_payload_bytes_per_second as f64
    }

    pub(crate) fn payload_bytes_for_audio_seconds(self, audio_seconds: f64) -> f64 {
        audio_seconds * self.pcm_payload_bytes_per_second as f64
    }
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
