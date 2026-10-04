use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::Instrument;

use crate::math::recursive_least_squares::RecursiveLeastSquares;
use crate::tts::{
    AudioOutput, SegmentReceiveUncertainty, StreamAudioFormat, StreamTimingProfile, Voice,
    VoiceDetail, VoiceError,
};

const STREAM_CHUNK_QUEUE_CAPACITY: usize = 8;
// Current VOICEVOX stream layout used by startup buffering and receive prediction.
const VOICEVOX_STREAM_AUDIO_FORMAT: StreamAudioFormat = StreamAudioFormat {
    container_header_bytes: 44,
    pcm_payload_bytes_per_second: 48_000,
};

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

#[derive(Deserialize)]
struct RawAccentPhrase {
    moras: Vec<RawMora>,
    pause_mora: Option<RawMora>,
}

#[derive(Deserialize)]
struct RawMora {
    consonant_length: Option<f64>,
    vowel_length: f64,
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

    fn estimated_audio_duration(&self) -> Result<std::time::Duration, VoiceError> {
        let phrases: Vec<RawAccentPhrase> = serde_json::from_str(self.accent_phrases.get())
            .map_err(|error| {
                VoiceError::Api(anyhow::anyhow!(
                    "Failed to parse VOICEVOX accent phrases for audio duration: {error}"
                ))
            })?;
        let phrase_seconds: f64 = phrases
            .iter()
            .map(|phrase| {
                phrase
                    .moras
                    .iter()
                    .map(|mora| mora.consonant_length.unwrap_or(0.0) + mora.vowel_length)
                    .sum::<f64>()
                    + phrase.pause_mora.as_ref().map_or(0.0, |mora| {
                        mora.consonant_length.unwrap_or(0.0) + mora.vowel_length
                    })
            })
            .sum();
        let pause_seconds =
            self.pause_length.unwrap_or(0.0) * self.pause_length_scale.unwrap_or(1.0);
        let total_seconds = phrase_seconds / self.speed_scale
            + pause_seconds
            + self.pre_phoneme_length
            + self.post_phoneme_length;
        if !total_seconds.is_finite() || total_seconds <= 0.0 {
            return Err(VoiceError::Api(anyhow::anyhow!(
                "VOICEVOX returned an invalid estimated audio duration: {total_seconds}"
            )));
        }
        let duration = std::time::Duration::try_from_secs_f64(total_seconds).map_err(|error| {
            VoiceError::Api(anyhow::anyhow!(
                "VOICEVOX estimated audio duration is out of range: {error}"
            ))
        })?;
        if duration.is_zero() {
            return Err(VoiceError::Api(anyhow::anyhow!(
                "VOICEVOX estimated audio duration rounded to zero"
            )));
        }
        Ok(duration)
    }
}

/// minimum client for Voicevox
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: reqwest::Url,
    request_timeout: std::time::Duration,
    streaming_synthesis: bool,
    buffer_sigma: f64,
    segment_length: f64,
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        base_url: reqwest::Url,
        request_timeout: std::time::Duration,
        streaming_synthesis: bool,
        buffer_sigma: f64,
        segment_length: f64,
    ) -> Client {
        Client {
            http,
            base_url,
            request_timeout,
            streaming_synthesis,
            buffer_sigma,
            segment_length,
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

    async fn synthesis(
        &self,
        speaker: i32,
        audio_query: LazyAudioQuery,
    ) -> anyhow::Result<Vec<u8>> {
        let url = self.base_url.join("/synthesis")?;
        let request = self
            .http
            .post(url)
            .query(&[("speaker", speaker.to_string())])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "audio/wav")
            .json(&audio_query);
        let response = tokio::time::timeout(self.request_timeout, request.send())
            .await??
            .error_for_status()?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
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
    streaming_receive_model: Arc<Mutex<StreamingReceiveModel>>,
}

/// RLS estimates for request receive time and EMA uncertainty for
/// segment-to-segment receive-time variation.
struct StreamingReceiveModel {
    rls: RecursiveLeastSquares<2>,
    segment_uncertainty: SegmentUncertaintyModel,
}

#[derive(Clone, Copy)]
struct StreamingReceivePrediction {
    total_receive_time: std::time::Duration,
    segment_uncertainty: SegmentReceiveUncertainty,
}

#[derive(Clone, Copy)]
enum SegmentUncertaintyModel {
    Warmup {
        error_variance_secs2: f64,
        samples_seen: u64,
    },
    Estimated {
        error_variance_secs2: f64,
    },
}

impl SegmentUncertaintyModel {
    fn error_variance_secs2(self) -> f64 {
        match self {
            Self::Warmup {
                error_variance_secs2,
                ..
            }
            | Self::Estimated {
                error_variance_secs2,
            } => error_variance_secs2,
        }
    }

    fn observe(&mut self, squared_error: f64, alpha: f64, min_samples: u64) {
        let error_variance_secs2 = ema(self.error_variance_secs2(), squared_error, alpha);
        *self = match *self {
            Self::Warmup { samples_seen, .. } => {
                let samples_seen = samples_seen.saturating_add(1);
                if samples_seen >= min_samples {
                    Self::Estimated {
                        error_variance_secs2,
                    }
                } else {
                    Self::Warmup {
                        error_variance_secs2,
                        samples_seen,
                    }
                }
            }
            Self::Estimated { .. } => Self::Estimated {
                error_variance_secs2,
            },
        };
    }
}

impl StreamingReceiveModel {
    const INITIAL_COEFFICIENTS: [f64; 2] = [1.0, 0.0];
    const INITIAL_COVARIANCE: [[f64; 2]; 2] = [[100.0, 0.0], [0.0, 100.0]];
    const FORGETTING_FACTOR: f64 = 0.9;
    /// Ignore short chunks when measuring the receive interval of full segments.
    const MIN_SEGMENT_CHUNK_COMPLETENESS: f64 = 0.9;
    const UNCERTAINTY_ALPHA: f64 = 0.05;
    const MIN_UNCERTAINTY_SAMPLES: u64 = 5;

    fn new() -> Self {
        Self {
            rls: RecursiveLeastSquares::new(
                Self::INITIAL_COEFFICIENTS,
                Self::INITIAL_COVARIANCE,
                Self::FORGETTING_FACTOR,
            ),
            segment_uncertainty: SegmentUncertaintyModel::Warmup {
                error_variance_secs2: 0.0,
                samples_seen: 0,
            },
        }
    }

    fn predict(
        &self,
        audio_duration: std::time::Duration,
    ) -> Result<StreamingReceivePrediction, VoiceError> {
        let estimated_secs = self
            .rls
            .predict(Self::receive_time_features(audio_duration.as_secs_f64()))
            .max(0.0);
        let total_receive_time =
            std::time::Duration::try_from_secs_f64(estimated_secs).map_err(|error| {
                VoiceError::Api(anyhow::anyhow!(
                    "VOICEVOX receive-time prediction is out of range: {error}"
                ))
            })?;

        Ok(StreamingReceivePrediction {
            total_receive_time,
            segment_uncertainty: self.segment_uncertainty(),
        })
    }

    fn receive_time_features(audio_duration_secs: f64) -> [f64; 2] {
        [audio_duration_secs, 1.0]
    }

    fn observe_completed_request(&mut self, audio_duration_secs: f64, receive_time_secs: f64) {
        if !audio_duration_secs.is_finite()
            || audio_duration_secs <= 0.0
            || !receive_time_secs.is_finite()
            || receive_time_secs <= 0.0
        {
            return;
        }

        let _ = self.rls.update(
            Self::receive_time_features(audio_duration_secs),
            receive_time_secs,
        );
    }

    fn segment_uncertainty(&self) -> SegmentReceiveUncertainty {
        match self.segment_uncertainty {
            SegmentUncertaintyModel::Warmup { .. } => SegmentReceiveUncertainty::Warmup,
            SegmentUncertaintyModel::Estimated {
                error_variance_secs2,
            } => SegmentReceiveUncertainty::Estimated {
                standard_deviation_secs: error_variance_secs2.max(0.0).sqrt(),
            },
        }
    }

    fn observe_segment_interval(&mut self, interval_secs: f64, audio_segment_secs: f64) {
        if !interval_secs.is_finite()
            || interval_secs <= 0.0
            || !audio_segment_secs.is_finite()
            || audio_segment_secs <= 0.0
        {
            return;
        }

        let expected_interval = self.rls.coefficients()[0] * audio_segment_secs;
        let segment_error = interval_secs - expected_interval;
        self.segment_uncertainty.observe(
            segment_error * segment_error,
            Self::UNCERTAINTY_ALPHA,
            Self::MIN_UNCERTAINTY_SAMPLES,
        );
    }
}

fn ema(previous: f64, sample: f64, alpha: f64) -> f64 {
    (1.0 - alpha) * previous + alpha * sample
}

impl VoicevoxVoice {
    pub fn new(client: Client, config: VoicevoxVoiceConfig) -> VoicevoxVoice {
        let identifier = Self::build_identifier(&config);
        Self {
            identifier,
            client,
            config,
            // Keep a separate receive-time model per configured voice.
            streaming_receive_model: Arc::new(Mutex::new(StreamingReceiveModel::new())),
        }
    }

    async fn buffered_output(
        &self,
        audio_query: LazyAudioQuery,
    ) -> Result<AudioOutput, VoiceError> {
        let bytes = self
            .client
            .synthesis(self.config.speaker_id, audio_query)
            .await
            .map_err(VoiceError::Api)?;
        Ok(AudioOutput::Buffered(bytes.into()))
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

        if !self.client.streaming_synthesis {
            return self.buffered_output(audio_query).await;
        }

        // `segment_length` is the configured target duration, in seconds, for
        // each Engine-generated segment.
        let segment_length = self.client.segment_length;
        tracing::debug!(
            speaker_id = self.config.speaker_id,
            segment_length_secs = segment_length,
            "Using configured VOICEVOX streaming segment length"
        );

        let startup_prediction = audio_query.estimated_audio_duration().and_then(|duration| {
            self.streaming_receive_model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .predict(duration)
                .map(|prediction| (duration, prediction))
        });
        let (estimated_audio_duration, prediction) = match startup_prediction {
            Ok(prediction) => prediction,
            Err(_) => return self.buffered_output(audio_query).await,
        };
        let span = tracing::info_span!(
            "voicevox_stream",
            speaker_id = self.config.speaker_id,
            segment_length_secs = segment_length
        );
        let request_started = std::time::Instant::now();
        let request_started_at = tokio::time::Instant::now();

        let response = self
            .client
            .streaming_synthesis(self.config.speaker_id, segment_length, audio_query)
            .instrument(span.clone())
            .await
            .map_err(VoiceError::Api)?;

        let (tx, chunks) = mpsc::channel(STREAM_CHUNK_QUEUE_CAPACITY);
        let idle_timeout = self.client.request_timeout;
        let expected_bytes = response.content_length();
        let request_elapsed = request_started.elapsed();
        let receive_model = self.streaming_receive_model.clone();
        let stream_span = span.clone();
        tokio::spawn(async move {
            let mut body = response.bytes_stream();
            let mut received_bytes = 0u64;
            // Count request/header time and time actively waiting for HTTP
            // body data, but exclude time blocked forwarding into the bounded
            // consumer channel. Otherwise Session's own startup buffering
            // would bias the EMA upward.
            let mut measured_receive_time = request_elapsed;
            let mut saw_pcm_chunk = false;
            loop {
                let wait_started = std::time::Instant::now();
                let next = match tokio::time::timeout(idle_timeout, body.next()).await {
                    Ok(next) => next,
                    Err(error) => {
                        let error = VoiceError::Api(error.into());
                        tracing::warn!(?error, "VOICEVOX streaming response stalled");
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                };
                let receive_wait = wait_started.elapsed();
                measured_receive_time += receive_wait;
                let Some(result) = next else {
                    if expected_bytes.is_some_and(|expected| expected != received_bytes) {
                        let error = VoiceError::Api(anyhow::anyhow!(
                            "VOICEVOX stream ended after {received_bytes} bytes; expected {expected_bytes:?}"
                        ));
                        tracing::warn!(?error, "VOICEVOX streaming response was truncated");
                        let _ = tx.send(Err(error)).await;
                    } else {
                        let t = measured_receive_time.as_secs_f64();
                        let mut model = receive_model
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let d = estimated_audio_duration.as_secs_f64();
                        model.observe_completed_request(d, t);
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
                        if !bytes.is_empty() {
                            let pcm_chunk_bytes = VOICEVOX_STREAM_AUDIO_FORMAT
                                .payload_bytes_in_chunk(received_bytes, bytes.len() as u64);
                            if saw_pcm_chunk {
                                // Later full-size body chunks represent the
                                // fixed-length synthesized segments. Exclude
                                // the first interval, which includes startup.
                                let full_segment_bytes = VOICEVOX_STREAM_AUDIO_FORMAT
                                    .payload_bytes_for_audio_seconds(segment_length);
                                if pcm_chunk_bytes as f64
                                    >= full_segment_bytes
                                        * StreamingReceiveModel::MIN_SEGMENT_CHUNK_COMPLETENESS
                                {
                                    let mut model = receive_model
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    model.observe_segment_interval(
                                        receive_wait.as_secs_f64(),
                                        VOICEVOX_STREAM_AUDIO_FORMAT
                                            .audio_seconds_for_payload_bytes(pcm_chunk_bytes),
                                    );
                                }
                            }
                            if pcm_chunk_bytes > 0 {
                                saw_pcm_chunk = true;
                            }
                        }
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
        }.instrument(stream_span));

        Ok(AudioOutput::Stream {
            chunks,
            timing: StreamTimingProfile {
                span,
                request_started_at,
                estimated_total_receive_time: prediction.total_receive_time,
                total_audio_playback_duration: estimated_audio_duration,
                audio_format: VOICEVOX_STREAM_AUDIO_FORMAT,
                receive_uncertainty: prediction.segment_uncertainty,
                buffer_sigma: self.client.buffer_sigma,
                chunk_audio_duration: std::time::Duration::from_secs_f64(segment_length),
            },
        })
    }
}
