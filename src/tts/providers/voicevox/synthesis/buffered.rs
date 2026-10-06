use crate::tts::providers::voicevox::{
    Client, VoicevoxVoiceConfig, client::LazyAudioQuery, synthesis::VoicevoxSynthesis,
};
use crate::tts::{AudioOutput, VoiceError};

pub(in crate::tts::providers::voicevox) struct BufferedSynthesis;

#[async_trait::async_trait]
impl VoicevoxSynthesis for BufferedSynthesis {
    async fn generate(
        &self,
        client: &Client,
        config: &VoicevoxVoiceConfig,
        audio_query: LazyAudioQuery,
    ) -> Result<AudioOutput, VoiceError> {
        let bytes = client
            .synthesis(config.speaker_id, audio_query)
            .await
            .map_err(VoiceError::Api)?;
        Ok(AudioOutput::Buffered(bytes.into()))
    }
}
