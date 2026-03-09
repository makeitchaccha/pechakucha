use crate::tts::{DISCORD_SAMPLE_RATE, Voice, VoiceDetail, VoiceError};
use async_trait::async_trait;
use google_cloud_texttospeech_v1::client::TextToSpeech;
use google_cloud_texttospeech_v1::model::{
    AudioConfig, AudioEncoding, SsmlVoiceGender, SynthesisInput, VoiceSelectionParams,
};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all(deserialize = "snake_case"))]
pub enum GenderConfig {
    #[default]
    Unspecified,
    Male,
    Female,
    Neutral,
}

impl From<GenderConfig> for SsmlVoiceGender {
    fn from(g: GenderConfig) -> Self {
        match g {
            GenderConfig::Unspecified => SsmlVoiceGender::Unspecified,
            GenderConfig::Male => SsmlVoiceGender::Male,
            GenderConfig::Female => SsmlVoiceGender::Female,
            GenderConfig::Neutral => SsmlVoiceGender::Neutral,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all(deserialize = "snake_case"))]
pub enum Encoding {
    #[default]
    Linear16,
    Mp3,
    OggOpus,
    Mulaw,
    Alaw,
    // Pcm, // omit since no header format requires effort to support.
    M4A,
}

impl From<Encoding> for AudioEncoding {
    fn from(e: Encoding) -> Self {
        match e {
            Encoding::Linear16 => AudioEncoding::Linear16,
            Encoding::Mp3 => AudioEncoding::Mp3,
            Encoding::OggOpus => AudioEncoding::OggOpus,
            Encoding::Mulaw => AudioEncoding::Mulaw,
            Encoding::Alaw => AudioEncoding::Alaw,
            Encoding::M4A => AudioEncoding::M4A,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GoogleCloudVoiceConfig {
    pub language_code: String,
    pub name: Option<String>,
    pub ssml_gender: Option<GenderConfig>,
    pub model_name: Option<String>,
    pub speaking_rate: Option<f32>,
    pub pitch: Option<f64>,
    pub volume_gain_db: Option<f64>,
    pub encoding: Option<Encoding>,
}

impl GoogleCloudVoiceConfig {
    pub fn generate_default_detail(&self, key: &str) -> VoiceDetail {
        VoiceDetail {
            name: key.to_owned(),
            provider: "Google Cloud".to_owned(),
            description: None,
        }
    }
}

impl From<GoogleCloudVoiceConfig> for (VoiceSelectionParams, AudioConfig) {
    fn from(c: GoogleCloudVoiceConfig) -> (VoiceSelectionParams, AudioConfig) {
        let params = VoiceSelectionParams::new()
            .set_language_code(&c.language_code)
            .set_name(c.name.unwrap_or_default())
            .set_ssml_gender(c.ssml_gender.unwrap_or_default())
            .set_model_name(c.model_name.unwrap_or_default());

        let audio = AudioConfig::new()
            .set_audio_encoding(AudioEncoding::Pcm)
            .set_speaking_rate(c.speaking_rate.unwrap_or_default())
            .set_pitch(c.pitch.unwrap_or_default())
            .set_volume_gain_db(c.volume_gain_db.unwrap_or_default())
            .set_sample_rate_hertz(DISCORD_SAMPLE_RATE);

        (params, audio)
    }
}

pub struct GoogleCloudVoice {
    identifier: String,
    client: TextToSpeech,
    voice_selection_params: VoiceSelectionParams,
    audio_config: AudioConfig,
}

impl GoogleCloudVoice {
    pub fn new(client: TextToSpeech, config: GoogleCloudVoiceConfig) -> Self {
        let (voice_selection_params, audio_config) = config.into();
        let identifier = Self::build_identifier(&voice_selection_params, &audio_config);
        Self {
            identifier,
            client,
            voice_selection_params,
            audio_config,
        }
    }

    fn build_identifier(
        voice_selection_params: &VoiceSelectionParams,
        audio_config: &AudioConfig,
    ) -> String {
        format!(
            "google_cloud-(lang:{},name:{},gender:{},model:{},encoding:{},speaking_rate:{},pitch:{},gain:{},sample_rate_hz:{})",
            voice_selection_params.language_code,
            voice_selection_params.name,
            voice_selection_params.ssml_gender,
            voice_selection_params.model_name,
            audio_config.audio_encoding,
            audio_config.speaking_rate,
            audio_config.pitch,
            audio_config.volume_gain_db,
            audio_config.sample_rate_hertz,
        )
    }
}

#[async_trait]
impl Voice for GoogleCloudVoice {
    fn identifier(&self) -> &str {
        self.identifier.as_str()
    }

    fn language(&self) -> &str {
        self.voice_selection_params.language_code.as_str()
    }

    async fn generate(&self, text: &str) -> Result<Vec<u8>, VoiceError> {
        tracing::debug!("google cloud voice requested to generate: {}", text);
        let response = match self
            .client
            .synthesize_speech()
            .set_voice(self.voice_selection_params.clone())
            .set_audio_config(self.audio_config.clone())
            .set_input(SynthesisInput::new().set_text(text))
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => return Err(VoiceError::Api(err.into())),
        };

        Ok(response.audio_content.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use google_cloud_texttospeech_v1::model::{
        AudioConfig, AudioEncoding, SsmlVoiceGender, VoiceSelectionParams,
    };

    #[test]
    fn test_identifier_generation() {
        let voice_params = VoiceSelectionParams::new()
            .set_language_code("ja-JP")
            .set_name("ja-JP-Wavenet-A")
            .set_ssml_gender(SsmlVoiceGender::Female)
            .set_model_name("default");

        let audio_config = AudioConfig::new()
            .set_audio_encoding(AudioEncoding::Pcm)
            .set_speaking_rate(1.5)
            .set_pitch(-2)
            .set_volume_gain_db(3)
            .set_sample_rate_hertz(48000)
            .set_volume_gain_db(3);

        assert_eq!(
            GoogleCloudVoice::build_identifier(&voice_params, &audio_config),
            "google_cloud-(lang:ja-JP,name:ja-JP-Wavenet-A,gender:FEMALE,model:default,encoding:PCM,speaking_rate:1.5,pitch:-2,gain:3,sample_rate_hz:48000)"
        );
    }
}
