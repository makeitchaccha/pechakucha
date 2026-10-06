mod buffered;
mod streaming;

use crate::tts::providers::voicevox::{Client, VoicevoxVoiceConfig, client::LazyAudioQuery};
use crate::tts::{AudioOutput, VoiceError};
#[derive(Clone, Copy, Debug)]
pub enum VoicevoxSynthesisFactory {
    Buffered,
    Streaming {
        buffer_sigma: f64,
        segment_length: f64,
    },
}

impl VoicevoxSynthesisFactory {
    pub fn buffered() -> Self {
        Self::Buffered
    }

    pub fn streaming(buffer_sigma: f64, segment_length: f64) -> Self {
        Self::Streaming {
            buffer_sigma,
            segment_length,
        }
    }

    pub(super) fn create(self) -> Box<dyn VoicevoxSynthesis> {
        match self {
            Self::Buffered => Box::new(buffered::BufferedSynthesis),
            Self::Streaming {
                buffer_sigma,
                segment_length,
            } => Box::new(streaming::StreamingSynthesis::new(
                buffer_sigma,
                segment_length,
            )),
        }
    }
}

#[async_trait::async_trait]
pub(super) trait VoicevoxSynthesis: Send + Sync {
    async fn generate(
        &self,
        client: &Client,
        config: &VoicevoxVoiceConfig,
        audio_query: LazyAudioQuery,
    ) -> Result<AudioOutput, VoiceError>;
}
