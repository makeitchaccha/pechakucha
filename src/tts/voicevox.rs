use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::tts::{Voice, VoiceDetail, VoiceError};

#[derive(Serialize, Deserialize)]
struct LazyAudioQuery {
    accent_phrases: Box<serde_json::value::RawValue>,
    #[serde(rename = "speedScale")]
    speed_scale: f64,
    #[serde(rename = "pitchScale")]
    pitch_scale: f64,
    #[serde(rename = "intonationScale")]
    intonation_scale: f64,
    #[serde(rename = "volumeScale")]
    volume_scale: f64,
    #[serde(rename = "prePhonemeLength")]
    pre_phoneme_length: f64,
    #[serde(rename = "postPhonemeLength")]
    post_phoneme_length: f64,
    #[serde(rename = "pauseLength")]
    pause_length: Option<f64>,
    #[serde(rename = "pauseLengthScale")]
    pause_length_scale: Option<f64>,
    #[serde(rename = "outputSamplingRate")]
    output_sampling_rate: u32,
    #[serde(rename = "outputStereo")]
    output_stereo: bool,
    kana: Option<String>,
}

impl LazyAudioQuery {
    pub fn apply_config(&mut self, config: &VoicevoxVoiceConfig) {
        if let Some(s) = config.speed_scale {
            self.speed_scale = s;
        }
        if let Some(p) = config.pitch_scale {
            self.pitch_scale = p;
        }
        if let Some(i) = config.intonation_scale {
            self.intonation_scale = i;
        }
        if let Some(v) = config.volume_scale {
            self.volume_scale = v;
        }
        if let Some(l) = config.pre_phoneme_length {
            self.pre_phoneme_length = l;
        }
        if let Some(l) = config.post_phoneme_length {
            self.post_phoneme_length = l;
        }
    }
}

/// minimum client for Voicevox
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: reqwest::Url,
}

impl Client {
    pub fn new(http: reqwest::Client, base_url: reqwest::Url) -> Client {
        Client { http, base_url }
    }

    async fn audio_query(&self, text: &str, speaker: i32) -> anyhow::Result<LazyAudioQuery> {
        let url = self.base_url.join("/audio_query")?;
        let res = self
            .http
            .post(url)
            .query(&[("text", text), ("speaker", &speaker.to_string())])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?
            .error_for_status()?;

        let audio_query: LazyAudioQuery = res.json().await?;
        Ok(audio_query)
    }

    async fn synthesis(
        &self,
        speaker: i32,
        audio_query: LazyAudioQuery,
    ) -> anyhow::Result<Vec<u8>> {
        let url = self.base_url.join("/synthesis")?;
        let res = self
            .http
            .post(url)
            .query(&[("speaker", &speaker.to_string())])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "audio/wav")
            .json(&audio_query)
            .send()
            .await?
            .error_for_status()?;

        Ok(res.bytes().await?.to_vec())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoicevoxVoiceConfig {
    pub speaker_id: i32,
    pub speed_scale: Option<f64>,
    pub pitch_scale: Option<f64>,
    pub intonation_scale: Option<f64>,
    pub volume_scale: Option<f64>,
    pub pre_phoneme_length: Option<f64>,
    pub post_phoneme_length: Option<f64>,
}

impl VoicevoxVoiceConfig {
    pub fn generate_default_detail(&self, key: &str) -> VoiceDetail {
        VoiceDetail {
            name: key.to_string(),
            provider: "VOICEVOX".to_string(),
            description: None,
        }
    }
}

pub struct VoicevoxVoice {
    identifier: String,
    client: Client,
    config: VoicevoxVoiceConfig,
}

impl VoicevoxVoice {
    pub fn new(client: Client, config: VoicevoxVoiceConfig) -> VoicevoxVoice {
        let identifier = Self::build_identifier(&config);
        Self {
            identifier,
            client,
            config,
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

    async fn generate(&self, text: &str) -> Result<Vec<u8>, VoiceError> {
        let mut audio_query = self
            .client
            .audio_query(text, self.config.speaker_id)
            .await
            .map_err(VoiceError::Api)?;

        audio_query.apply_config(&self.config);

        let res_synthesis = self
            .client
            .synthesis(self.config.speaker_id, audio_query)
            .await
            .map_err(VoiceError::Api)?;

        Ok(res_synthesis)
    }
}
