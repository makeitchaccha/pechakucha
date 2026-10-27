use crate::tts::providers::voicevox::{
    Client, VoicevoxVoiceConfig, client::LazyAudioQuery, synthesis::VoicevoxSynthesis,
};
mod model;
use crate::tts::{AudioOutput, StreamAudioFormat, StreamTimingProfile, VoiceError};
use model::StreamingReceiveModel;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::Instrument;

const STREAM_CHUNK_QUEUE_CAPACITY: usize = 8;
// Current VOICEVOX stream layout used by startup buffering and receive prediction.
const VOICEVOX_STREAM_AUDIO_FORMAT: StreamAudioFormat = StreamAudioFormat {
    container_header_bytes: 44,
    pcm_payload_bytes_per_second: 48_000,
};

pub(in crate::tts::providers::voicevox) struct StreamingSynthesis {
    buffer_sigma: f64,
    segment_length: f64,
}

impl StreamingSynthesis {
    pub(super) fn new(buffer_sigma: f64, segment_length: f64) -> Self {
        Self {
            buffer_sigma,
            segment_length,
        }
    }
}

#[async_trait::async_trait]
impl VoicevoxSynthesis for StreamingSynthesis {
    async fn generate(
        &self,
        client: &Client,
        config: &VoicevoxVoiceConfig,
        audio_query: LazyAudioQuery,
    ) -> Result<AudioOutput, VoiceError> {
        // `segment_length` is the configured target duration, in seconds, for
        // each Engine-generated segment.
        let segment_length = self.segment_length;
        let receive_model = StreamingReceiveModel::new();
        let span = tracing::info_span!(
            "voicevox_stream",
            speaker_id = config.speaker_id,
            segment_length_secs = segment_length
        )
        .or_current();

        let startup_prediction = span.in_scope(|| {
            audio_query
                .estimated_audio_duration()
                .and_then(|duration| {
                    receive_model
                        .predict(duration)
                        .map(|prediction| (duration, prediction))
                })
                .map(|(duration, prediction)| {
                    tracing::debug!(
                        estimated_audio_secs = duration.as_secs_f64(),
                        predicted_total_receive_secs = prediction.total_receive_time.as_secs_f64(),
                        r = prediction.receive_rate_secs_per_audio_sec,
                        b = prediction.request_overhead_secs,
                        "VOICEVOX receive-time model prediction"
                    );
                    (duration, prediction)
                })
        });
        let (estimated_audio_duration, prediction) = match startup_prediction {
            Ok(prediction) => prediction,
            Err(_) => {
                let bytes = client
                    .synthesis(config.speaker_id, audio_query)
                    .await
                    .map_err(VoiceError::Api)?;
                return Ok(AudioOutput::Buffered(bytes.into()));
            }
        };
        let receive_uncertainty = Arc::new(Mutex::new(prediction.segment_uncertainty));
        let estimated_total_receive_time = Arc::new(Mutex::new(prediction.total_receive_time));
        let request_started_at = tokio::time::Instant::now();

        let mut response = client
            .streaming_synthesis(config.speaker_id, segment_length, audio_query)
            .instrument(span.clone())
            .await
            .map_err(VoiceError::Api)?;

        let (tx, chunks) = mpsc::channel(STREAM_CHUNK_QUEUE_CAPACITY);
        let expected_bytes = response.content_length();
        let stream_span = span.clone();
        let receive_uncertainty_for_task = receive_uncertainty.clone();
        let estimated_total_for_task = estimated_total_receive_time.clone();
        tokio::spawn(async move {
            let mut receive_model = receive_model;
            let receive_uncertainty = receive_uncertainty_for_task;
            let estimated_total_receive_time = estimated_total_for_task;
            let mut received_bytes = 0u64;
            // Count request/header time and time actively waiting for HTTP
            // body data, but exclude time blocked forwarding into the bounded
            // consumer channel. Otherwise Session's own startup buffering
            // would bias the EMA upward.
            let mut saw_pcm_chunk = false;
            loop {
                let wait_started = std::time::Instant::now();
                let next = match response.next_chunk().await {
                    Ok(next) => next,
                    Err(error) => {
                        let error = VoiceError::Api(error);
                        tracing::warn!(?error, "VOICEVOX streaming response failed");
                        let _ = tx.send(Err(error)).await;
                        break;
                    }
                };
                let receive_wait = wait_started.elapsed();
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
                let bytes = result;
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
                            receive_model.observe_segment_interval(
                                receive_wait.as_secs_f64(),
                                VOICEVOX_STREAM_AUDIO_FORMAT
                                    .audio_seconds_for_payload_bytes(pcm_chunk_bytes),
                            );
                            let updated = receive_model.segment_uncertainty();
                            *receive_uncertainty
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) = updated;
                            if let Some(predicted_secs) = receive_model
                                .current_request_predicted_total_secs(estimated_audio_duration.as_secs_f64())
                            {
                                if let Ok(d) = std::time::Duration::try_from_secs_f64(predicted_secs) {
                                    *estimated_total_receive_time
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner) = d;
                                }
                            }
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
        }.instrument(stream_span.or_current()));

        Ok(AudioOutput::Stream {
            chunks,
            timing: StreamTimingProfile {
                request_started_at,
                estimated_total_receive_time: estimated_total_receive_time,
                total_audio_playback_duration: estimated_audio_duration,
                audio_format: VOICEVOX_STREAM_AUDIO_FORMAT,
                receive_uncertainty: receive_uncertainty,
                buffer_sigma: self.buffer_sigma,
                chunk_audio_duration: std::time::Duration::from_secs_f64(segment_length),
            },
        })
    }
}
