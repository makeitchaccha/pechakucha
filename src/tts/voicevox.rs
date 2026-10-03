use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::tts::{AudioOutput, StreamTimingProfile, Voice, VoiceDetail, VoiceError};

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
    request_timeout: std::time::Duration,
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        base_url: reqwest::Url,
        request_timeout: std::time::Duration,
    ) -> Client {
        Client {
            http,
            base_url,
            request_timeout,
        }
    }

    async fn audio_query(&self, text: &str, speaker: i32) -> anyhow::Result<LazyAudioQuery> {
        let url = self.base_url.join("/audio_query")?;
        let request = self
            .http
            .post(url)
            .query(&[("text", text), ("speaker", &speaker.to_string())])
            .header(reqwest::header::ACCEPT, "application/json");
        let audio_query = tokio::time::timeout(self.request_timeout, async {
            request
                .send()
                .await?
                .error_for_status()?
                .json::<LazyAudioQuery>()
                .await
                .map_err(anyhow::Error::from)
        })
        .await??;
        Ok(audio_query)
    }

    async fn streaming_synthesis(
        &self,
        speaker: i32,
        segment_length: f64,
        audio_query: LazyAudioQuery,
    ) -> anyhow::Result<reqwest::Response> {
        let url = self.base_url.join("/streaming_synthesis")?;
        let request = self
            .http
            .post(url)
            .query(&[
                ("speaker", speaker.to_string()),
                ("segment_length", segment_length.to_string()),
            ])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "audio/wav")
            .json(&audio_query);
        let response = tokio::time::timeout(self.request_timeout, request.send())
            .await??
            .error_for_status()?;

        Ok(response)
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

    async fn generate(&self, text: &str) -> Result<AudioOutput, VoiceError> {
        let mut audio_query = self
            .client
            .audio_query(text, self.config.speaker_id)
            .await
            .map_err(VoiceError::Api)?;

        audio_query.apply_config(&self.config);

        // `segment_length` is the target duration, in seconds, for each
        // Engine-generated segment. Measurements showed 3 seconds gave a
        // useful balance between first audio latency and total synthesis time.
        let segment_length = 3.0;
        tracing::debug!(
            speaker_id = self.config.speaker_id,
            segment_length_secs = segment_length,
            "Using fixed VOICEVOX streaming segment length"
        );

        let response = self
            .client
            .streaming_synthesis(self.config.speaker_id, segment_length, audio_query)
            .await
            .map_err(VoiceError::Api)?;

        let (tx, chunks) = mpsc::channel(8);
        let idle_timeout = self.client.request_timeout;
        let expected_bytes = response.content_length();
        tokio::spawn(async move {
            let mut body = response.bytes_stream();
            let mut received_bytes = 0u64;
            loop {
                let next = match tokio::time::timeout(idle_timeout, body.next()).await {
                    Ok(next) => next,
                    Err(error) => {
                        let error = VoiceError::Api(error.into());
                        tracing::warn!(?error, "VOICEVOX streaming response stalled");
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                };
                let Some(result) = next else {
                    if expected_bytes.is_some_and(|expected| expected != received_bytes) {
                        let error = VoiceError::Api(anyhow::anyhow!(
                            "VOICEVOX stream ended after {received_bytes} bytes; expected {expected_bytes:?}"
                        ));
                        tracing::warn!(?error, "VOICEVOX streaming response was truncated");
                        let _ = tx.send(Err(error)).await;
                    } else {
                        tracing::debug!(
                            received_bytes,
                            expected_bytes,
                            "VOICEVOX stream completed"
                        );
                    }
                    break;
                };
                match result {
                    Ok(bytes) => {
                        received_bytes += bytes.len() as u64;
                        if tx.send(Ok(bytes)).await.is_err() {
                            tracing::debug!("VOICEVOX stream consumer was dropped");
                            break;
                        }
                    }
                    Err(error) => {
                        let error = VoiceError::Api(error.into());
                        tracing::warn!(?error, "VOICEVOX streaming response failed");
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                }
            }
        });

        Ok(AudioOutput::Stream {
            chunks,
            timing: StreamTimingProfile {
                first_audio_latency: None,
                chunk_audio_duration: Some(std::time::Duration::from_secs_f64(segment_length)),
                max_chunk_arrival: None,
                sample_count: 0,
            },
        })
    }
}
