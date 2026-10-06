mod client;
mod config;
pub(super) mod synthesis;

pub use client::Client;
pub use config::VoicevoxVoiceConfig;
pub use synthesis::VoicevoxSynthesisFactory;

use synthesis::VoicevoxSynthesis;

use async_trait::async_trait;

use crate::tts::{AudioOutput, Voice, VoiceError};

pub struct VoicevoxVoice {
    identifier: String,
    client: Client,
    config: VoicevoxVoiceConfig,
    synthesis: Box<dyn VoicevoxSynthesis>,
}

impl VoicevoxVoice {
    pub fn new(
        client: Client,
        config: VoicevoxVoiceConfig,
        synthesis_factory: VoicevoxSynthesisFactory,
    ) -> VoicevoxVoice {
        let identifier = Self::build_identifier(&config);
        let synthesis = synthesis_factory.create();
        Self {
            identifier,
            client,
            config,
            synthesis,
        }
    }

    fn build_identifier(config: &VoicevoxVoiceConfig) -> String {
        format!(
            "voicevox-({})",
            serde_json::to_string(config).expect("failed to build identifier")
        )
    }
}

#[async_trait]
impl Voice for VoicevoxVoice {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    fn language(&self) -> &str {
        "ja-JP"
    }

    async fn generate(&self, text: &str) -> Result<AudioOutput, VoiceError> {
        let mut audio_query = self
            .client
            .audio_query(text, self.config.speaker_id)
            .await
            .map_err(VoiceError::Api)?;

        audio_query.apply_config(&self.config);

        self.synthesis
            .generate(&self.client, &self.config, audio_query)
            .await
    }
}
