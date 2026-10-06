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
    receive_model: Arc<Mutex<StreamingReceiveModel>>,
    buffer_sigma: f64,
    segment_length: f64,
}

impl StreamingSynthesis {
    pub(super) fn new(buffer_sigma: f64, segment_length: f64) -> Self {
        Self {
            receive_model: Arc::new(Mutex::new(StreamingReceiveModel::new())),
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
                    self.receive_model
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let request_started = std::time::Instant::now();
        let request_started_at = tokio::time::Instant::now();

        let mut response = client
            .streaming_synthesis(config.speaker_id, segment_length, audio_query)
            .instrument(span.clone())
            .await
            .map_err(VoiceError::Api)?;

        let (tx, chunks) = mpsc::channel(STREAM_CHUNK_QUEUE_CAPACITY);
        let expected_bytes = response.content_length();
        let request_elapsed = request_started.elapsed();
        let receive_model = self.receive_model.clone();
        let stream_span = span.clone();
        tokio::spawn(async move {
            let mut received_bytes = 0u64;
            // Count request/header time and time actively waiting for HTTP
            // body data, but exclude time blocked forwarding into the bounded
            // consumer channel. Otherwise Session's own startup buffering
            // would bias the EMA upward.
            let mut measured_receive_time = request_elapsed;
            let mut saw_pcm_chunk = false;
            loop {
                let wait_started = std::time::Instant::now();
                let next = match response.next_chunk().await {
                    Ok(next) => next,
                    Err(error) => {
                        let error = VoiceError::Api(error.into());
                        tracing::warn!(?error, "VOICEVOX streaming response failed");
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
                        let d = estimated_audio_duration.as_secs_f64();
                        let update = receive_model
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .observe_completed_request(d, t);
                        if let Some(update) = update {
                            tracing::debug!(
                                audio_duration_secs = d,
                                predicted_total_receive_secs = update.predicted_receive_time_secs,
                                observed_total_receive_secs = t,
                                residual_secs = t - update.predicted_receive_time_secs,
                                r_before = update.r_before,
                                b_before = update.b_before,
                                r_after = update.r_after,
                                b_after = update.b_after,
                                "VOICEVOX receive-time RLS updated"
                            );
                        }
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
        }.instrument(stream_span.or_current()));

        Ok(AudioOutput::Stream {
            chunks,
            timing: StreamTimingProfile {
                request_started_at,
                estimated_total_receive_time: prediction.total_receive_time,
                total_audio_playback_duration: estimated_audio_duration,
                audio_format: VOICEVOX_STREAM_AUDIO_FORMAT,
                receive_uncertainty: prediction.segment_uncertainty,
                buffer_sigma: self.buffer_sigma,
                chunk_audio_duration: std::time::Duration::from_secs_f64(segment_length),
            },
        })
    }
}
