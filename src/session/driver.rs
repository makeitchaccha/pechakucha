use crate::session::SessionControl;
use crate::tts::{AudioOutput, StreamReceiveMeasurement, StreamTimingProfile};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use songbird::input::{AsyncAdapterStream, AsyncReadOnlySource, AudioStream, Input, LiveInput};
use songbird::{Call, CoreEvent, Event, EventContext, EventHandler, TrackEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::io::StreamReader;
use tracing::Instrument;

const VOICEVOX_PCM_BYTES_PER_SECOND: usize = 48_000;
const VOICEVOX_RING_BUFFER_SECS: usize = 20;

#[async_trait]
pub trait AudioDriver: Sync + Send {
    async fn enqueue(
        &self,
        audios: Vec<Vec<u8>>,
        utterance_done: mpsc::Sender<PlaybackResult>,
    ) -> anyhow::Result<()>;

    async fn enqueue_outputs(
        &self,
        outputs: Vec<AudioOutput>,
        utterance_done: mpsc::Sender<PlaybackResult>,
    ) -> anyhow::Result<()>;

    async fn leave(&self) -> anyhow::Result<()>;

    async fn subscribe_to_disconnect_event(&self, tx: mpsc::Sender<SessionControl>);
}

pub struct SongbirdDriver {
    pub call: Arc<Mutex<Call>>,
}

#[derive(Clone)]
pub struct PlaybackResult {
    utterance_finished: bool,
    stream_report: Option<StreamPlaybackReport>,
}

impl PlaybackResult {
    fn utterance_finished(stream_report: Option<StreamPlaybackReport>) -> Self {
        Self {
            utterance_finished: true,
            stream_report,
        }
    }

    fn stream_finished(stream_report: StreamPlaybackReport) -> Self {
        Self {
            utterance_finished: false,
            stream_report: Some(stream_report),
        }
    }

    pub fn is_utterance_finished(&self) -> bool {
        self.utterance_finished
    }

    pub fn log_prediction_result(&self) {
        if let Some(report) = &self.stream_report {
            report.log_playback_end();
        }
    }

    pub fn log_track_error(&self) {
        if let Some(report) = &self.stream_report {
            tracing::debug!(
                parent: &report.span,
                "Songbird reported a VOICEVOX track error during initialization or playback"
            );
        }
    }
}

struct SongbirdEventHandler<T: Send + Sync + Clone> {
    tx: mpsc::Sender<T>,
    result: T,
}

#[derive(Clone)]
struct StreamPlaybackReport {
    span: tracing::Span,
    request_started_at: Option<tokio::time::Instant>,
    predicted_receive_time: Option<Duration>,
    receive_measurement: Arc<std::sync::Mutex<Option<StreamReceiveMeasurement>>>,
}

impl StreamPlaybackReport {
    fn from_timing(timing: &StreamTimingProfile) -> Option<Self> {
        Some(Self {
            span: timing.span.clone()?,
            request_started_at: timing.request_started_at,
            predicted_receive_time: timing.estimated_total_receive_time,
            receive_measurement: timing.receive_measurement.clone(),
        })
    }

    fn log_playback_end(&self) {
        let playback_ended_at = tokio::time::Instant::now();
        let measurement = *self
            .receive_measurement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(measurement) = measurement else {
            tracing::debug!(
                parent: &self.span,
                playback_end_elapsed_secs = self
                    .request_started_at
                    .map(|start| rounded_secs(playback_ended_at - start)),
                "VOICEVOX playback ended without a complete receive measurement"
            );
            return;
        };

        let predicted_secs = self
            .predicted_receive_time
            .map(|duration| duration.as_secs_f64());
        let actual_secs = measurement.active_receive_time.as_secs_f64();
        let prediction_error_secs = predicted_secs.map(|predicted| actual_secs - predicted);
        tracing::info!(
            parent: &self.span,
            prediction_equation = %predicted_secs.map(|predicted| format!(
                "{actual_secs:.2}s - {predicted:.2}s = {:+.2}s",
                actual_secs - predicted
            )).unwrap_or_else(|| "unavailable".to_string()),
            predicted_receive_time_secs = predicted_secs.map(round_secs_f64),
            actual_receive_time_secs = rounded_secs(measurement.active_receive_time),
            prediction_error_secs = prediction_error_secs.map(round_secs_f64),
            receive_to_playback_end_secs = rounded_secs(
                playback_ended_at.saturating_duration_since(measurement.completed_at)
            ),
            playback_end_elapsed_secs = self
                .request_started_at
                .map(|start| rounded_secs(playback_ended_at - start)),
            "VOICEVOX playback ended; receive-time prediction evaluated"
        );
    }
}
#[async_trait]
impl<T: Send + Sync + Clone> EventHandler for SongbirdEventHandler<T> {
    async fn act(&self, _: &EventContext<'_>) -> Option<Event> {
        let _ = self.tx.send(self.result.clone()).await;
        None
    }
}

#[async_trait]
impl AudioDriver for SongbirdDriver {
    async fn enqueue(
        &self,
        data: Vec<Vec<u8>>,
        utterance_done: mpsc::Sender<PlaybackResult>,
    ) -> anyhow::Result<()> {
        let mut call = self.call.lock().await;
        let last_index = data.len().saturating_sub(1);
        for (index, audio) in data.into_iter().enumerate() {
            let track = call.enqueue_input(audio.into()).await;
            if index == last_index {
                track
                    .add_event(
                        Event::Track(TrackEvent::End),
                        SongbirdEventHandler {
                            tx: utterance_done.clone(),
                            result: PlaybackResult::utterance_finished(None),
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to subscribe to utterance end: {e}"))?;
            }
        }
        Ok(())
    }

    async fn enqueue_outputs(
        &self,
        outputs: Vec<AudioOutput>,
        utterance_done: mpsc::Sender<PlaybackResult>,
    ) -> anyhow::Result<()> {
        let mut inputs = Vec::with_capacity(outputs.len());
        for output in outputs {
            let (input, stream_report) = match output {
                AudioOutput::Buffered(bytes) => (bytes.to_vec().into(), None),
                AudioOutput::Stream { chunks, timing } => {
                    let stream_report = StreamPlaybackReport::from_timing(&timing);
                    let fallback_wait =
                        estimated_remaining_receive_time(&timing, tokio::time::Instant::now())
                            .map(|duration| {
                                (duration.as_secs_f64() + receive_time_margin(&timing, 0))
                                    .clamp(0.25, 60.0)
                            })
                            .map(Duration::from_secs_f64)
                            .unwrap_or(Duration::from_millis(500));
                    let buffer_future = buffer_for_startup(chunks, &timing, fallback_wait);
                    let (buffered_chunks, chunks) = if let Some(span) = timing.span.clone() {
                        buffer_future.instrument(span).await
                    } else {
                        buffer_future.await
                    };
                    let stream = stream::iter(buffered_chunks)
                        .chain(stream::unfold(chunks, |mut receiver| async move {
                            receiver.recv().await.map(|chunk| (chunk, receiver))
                        }))
                        .map(|result| result.map_err(std::io::Error::other));
                    let reader = StreamReader::new(Box::pin(stream));
                    let source = AsyncReadOnlySource::new(reader);
                    let ring_buffer = VOICEVOX_PCM_BYTES_PER_SECOND * VOICEVOX_RING_BUFFER_SECS;
                    let adapter = AsyncAdapterStream::new(Box::new(source), ring_buffer);
                    let input = Input::Live(
                        LiveInput::Raw(AudioStream {
                            input: Box::new(adapter),
                        }),
                        None,
                    );
                    (input, stream_report)
                }
            };
            inputs.push((input, stream_report));
        }

        let mut call = self.call.lock().await;
        let last_index = inputs.len().saturating_sub(1);
        for (index, (input, stream_report)) in inputs.into_iter().enumerate() {
            let track = call.enqueue_input(input).await;
            let is_last = index == last_index;
            if is_last || stream_report.is_some() {
                let result = if is_last {
                    PlaybackResult::utterance_finished(stream_report.clone())
                } else {
                    PlaybackResult::stream_finished(
                        stream_report
                            .clone()
                            .expect("stream report required for non-final stream"),
                    )
                };
                track
                    .add_event(
                        Event::Track(TrackEvent::End),
                        SongbirdEventHandler {
                            tx: utterance_done.clone(),
                            result,
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to subscribe to playback end: {e}"))?;
                if let Some(stream_report) = stream_report {
                    track
                        .add_event(
                            Event::Track(TrackEvent::Error),
                            SongbirdEventHandler {
                                tx: utterance_done.clone(),
                                result: PlaybackResult {
                                    utterance_finished: false,
                                    stream_report: Some(stream_report),
                                },
                            },
                        )
                        .map_err(|e| {
                            anyhow::anyhow!("Failed to subscribe to playback error: {e}")
                        })?;
                }
            }
        }
        Ok(())
    }

    async fn leave(&self) -> anyhow::Result<()> {
        let mut call = self.call.lock().await;
        call.leave().await?;
        Ok(())
    }

    async fn subscribe_to_disconnect_event(&self, tx: mpsc::Sender<SessionControl>) {
        let mut call = self.call.lock().await;
        call.add_global_event(
            Event::Core(CoreEvent::DriverDisconnect),
            SongbirdEventHandler {
                tx,
                result: SessionControl::Disconnected,
            },
        );
    }
}

async fn buffer_for_startup(
    mut chunks: mpsc::Receiver<Result<bytes::Bytes, crate::tts::VoiceError>>,
    timing: &crate::tts::StreamTimingProfile,
    fallback_wait: Duration,
) -> (
    Vec<Result<bytes::Bytes, crate::tts::VoiceError>>,
    mpsc::Receiver<Result<bytes::Bytes, crate::tts::VoiceError>>,
) {
    const MAX_STARTUP_BUFFER: Duration = Duration::from_secs(60);
    const WAV_HEADER_BYTES: u64 = 44;

    let started = tokio::time::Instant::now();
    let max_deadline = started + MAX_STARTUP_BUFFER;
    let fallback_deadline = started + fallback_wait;
    let mut buffered = Vec::new();
    let mut received_bytes = 0u64;
    loop {
        if let (Some(remaining_receive), Some(total_playback_duration)) = (
            estimated_remaining_receive_time(timing, tokio::time::Instant::now()),
            timing.total_audio_playback_duration,
        ) {
            let pcm_bytes = received_bytes.saturating_sub(WAV_HEADER_BYTES);
            let error_margin = receive_time_margin(timing, pcm_bytes);
            let adjusted_remaining = remaining_receive.as_secs_f64() + error_margin;
            let playback_margin = total_playback_duration
                .saturating_sub(timing.chunk_audio_duration.unwrap_or_default())
                .as_secs_f64();
            let segment_duration = timing.chunk_audio_duration.unwrap_or_default();
            if pcm_bytes > 0 && adjusted_remaining < playback_margin {
                tracing::info!(
                    decision_equation = %startup_decision_equation(
                        remaining_receive.as_secs_f64(),
                        error_margin,
                        total_playback_duration.as_secs_f64(),
                        segment_duration.as_secs_f64(),
                    ),
                    margin_equation = %receive_time_margin_equation(timing, pcm_bytes),
                    pcm_ready = pcm_bytes > 0,
                    elapsed_since_request_secs = timing
                        .request_started_at
                        .map(|start| rounded_secs(tokio::time::Instant::now() - start)),
                    predicted_total_receive_time_secs =
                        timing.estimated_total_receive_time.map(rounded_secs),
                    estimated_remaining_receive_secs = rounded_secs(remaining_receive),
                    buffered_pcm_bytes = pcm_bytes,
                    total_audio_playback_secs = rounded_secs(total_playback_duration),
                    segment_duration_secs = timing.chunk_audio_duration.map(rounded_secs),
                    buffer_sigma = timing.buffer_sigma.unwrap_or_default(),
                    receive_seconds_per_audio_second =
                        round_secs_f64(timing.receive_speed_secs_per_audio_second),
                    prediction_bias_secs = round_secs_f64(timing.prediction_bias_secs),
                    segment_receive_interval_stddev_secs =
                        timing.receive_segment_stddev_secs.map(round_secs_f64),
                    segment_variability_margin_secs = round_secs_f64(error_margin),
                    remaining_segments_for_margin = remaining_segment_count(timing, pcm_bytes),
                    adjusted_remaining_receive_secs = round_secs_f64(adjusted_remaining),
                    playback_margin_secs = round_secs_f64(playback_margin),
                    decision_slack_secs = round_secs_f64(playback_margin - adjusted_remaining),
                    "VOICEVOX startup buffer reached the predicted playback window"
                );
                break;
            }
        } else if tokio::time::Instant::now() >= fallback_deadline {
            tracing::info!(
                buffered_bytes = received_bytes,
                fallback_wait_secs = rounded_secs(fallback_wait),
                "VOICEVOX startup buffer used fallback wait"
            );
            break;
        }

        if tokio::time::Instant::now() >= max_deadline {
            let remaining_receive =
                estimated_remaining_receive_time(timing, tokio::time::Instant::now());
            let playback_margin = timing.total_audio_playback_duration.map(|duration| {
                duration.saturating_sub(timing.chunk_audio_duration.unwrap_or_default())
            });
            let error_margin =
                receive_time_margin(timing, received_bytes.saturating_sub(WAV_HEADER_BYTES));
            let adjusted_remaining =
                remaining_receive.map(|remaining| remaining.as_secs_f64() + error_margin);
            let playback_margin_secs = playback_margin.map(|margin| margin.as_secs_f64());
            let predicted_deficit_secs = adjusted_remaining
                .zip(playback_margin_secs)
                .map(|(remaining, margin)| remaining - margin);
            let decision_equation = remaining_receive
                .zip(timing.total_audio_playback_duration)
                .map(|(remaining, total)| {
                    startup_decision_equation(
                        remaining.as_secs_f64(),
                        error_margin,
                        total.as_secs_f64(),
                        timing
                            .chunk_audio_duration
                            .unwrap_or_default()
                            .as_secs_f64(),
                    )
                })
                .unwrap_or_else(|| "unavailable".to_string());
            tracing::info!(
                decision_equation = %decision_equation,
                margin_equation = %receive_time_margin_equation(
                    timing,
                    received_bytes.saturating_sub(WAV_HEADER_BYTES),
                ),
                pcm_ready = received_bytes > WAV_HEADER_BYTES,
                elapsed_since_request_secs = timing
                    .request_started_at
                    .map(|start| rounded_secs(tokio::time::Instant::now() - start)),
                predicted_total_receive_time_secs =
                    timing.estimated_total_receive_time.map(rounded_secs),
                estimated_remaining_receive_secs = remaining_receive.map(rounded_secs),
                buffered_bytes = received_bytes,
                playback_margin_secs = playback_margin.map(rounded_secs),
                buffer_sigma = timing.buffer_sigma.unwrap_or_default(),
                segment_variability_margin_secs = round_secs_f64(error_margin),
                remaining_segments_for_margin = remaining_segment_count(
                    timing,
                    received_bytes.saturating_sub(WAV_HEADER_BYTES),
                ),
                adjusted_remaining_receive_secs = adjusted_remaining.map(round_secs_f64),
                predicted_deficit_secs = predicted_deficit_secs.map(round_secs_f64),
                "VOICEVOX stream did not reach its predicted playback window within 60 seconds; starting anyway"
            );
            break;
        }

        let poll_deadline =
            (tokio::time::Instant::now() + Duration::from_millis(50)).min(max_deadline);
        tokio::select! {
            biased;
            chunk = chunks.recv() => match chunk {
                Some(chunk) => {
                    if let Ok(bytes) = &chunk {
                        received_bytes = received_bytes.saturating_add(bytes.len() as u64);
                    }
                    let is_error = chunk.is_err();
                    buffered.push(chunk);
                    if is_error {
                        tracing::debug!(
                            elapsed_since_request_secs = timing.request_started_at.map(|start| rounded_secs(tokio::time::Instant::now() - start)),
                            buffered_bytes = received_bytes,
                            "Starting VOICEVOX playback with a stream error in the prefetched data"
                        );
                        break;
                    }
                },
                None => {
                    tracing::debug!(
                        elapsed_since_request_secs = timing.request_started_at.map(|start| rounded_secs(tokio::time::Instant::now() - start)),
                        predicted_total_receive_time_secs = timing.estimated_total_receive_time.map(rounded_secs),
                        buffered_bytes = received_bytes,
                        "VOICEVOX stream completed before startup buffer threshold"
                    );
                    break;
                },
            },
            _ = tokio::time::sleep_until(poll_deadline) => {},
        }
    }
    (buffered, chunks)
}

fn rounded_secs(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 100.0).round() / 100.0
}

fn round_secs_f64(seconds: f64) -> f64 {
    (seconds * 100.0).round() / 100.0
}

fn estimated_remaining_receive_time(
    timing: &crate::tts::StreamTimingProfile,
    now: tokio::time::Instant,
) -> Option<Duration> {
    let elapsed = now.checked_duration_since(timing.request_started_at?)?;
    Some(timing.estimated_total_receive_time?.saturating_sub(elapsed))
}

fn receive_time_margin(timing: &crate::tts::StreamTimingProfile, pcm_bytes: u64) -> f64 {
    const COLD_START_MARGIN_SECS: f64 = 3.0;

    let Some(standard_deviation) = timing.receive_segment_stddev_secs else {
        return COLD_START_MARGIN_SECS;
    };
    let segments = remaining_segment_count(timing, pcm_bytes);
    timing.buffer_sigma.unwrap_or_default() * standard_deviation * segments.sqrt()
}

fn remaining_segment_count(timing: &crate::tts::StreamTimingProfile, pcm_bytes: u64) -> f64 {
    const PCM_BYTES_PER_SECOND: f64 = 48_000.0;
    let Some(total_audio) = timing.total_audio_playback_duration else {
        return 0.0;
    };
    let segment_secs = timing
        .chunk_audio_duration
        .unwrap_or_default()
        .as_secs_f64();
    if segment_secs <= 0.0 {
        return 0.0;
    }
    ((total_audio.as_secs_f64() - pcm_bytes as f64 / PCM_BYTES_PER_SECOND).max(0.0) / segment_secs)
        .ceil()
}

fn receive_time_margin_equation(
    timing: &crate::tts::StreamTimingProfile,
    pcm_bytes: u64,
) -> String {
    const COLD_START_MARGIN_SECS: f64 = 3.0;

    let Some(standard_deviation) = timing.receive_segment_stddev_secs else {
        return format!("cold_start = {COLD_START_MARGIN_SECS:.2}s");
    };
    let sigma = timing.buffer_sigma.unwrap_or_default();
    let remaining_segments = remaining_segment_count(timing, pcm_bytes);
    format!(
        "{sigma:.2} * {standard_deviation:.2}s * sqrt({remaining_segments:.0}) = {:.2}s",
        sigma * standard_deviation * remaining_segments.sqrt()
    )
}

fn startup_decision_equation(
    remaining_secs: f64,
    margin_secs: f64,
    total_playback_secs: f64,
    segment_secs: f64,
) -> String {
    let adjusted_remaining = remaining_secs + margin_secs;
    let playback_margin = (total_playback_secs - segment_secs).max(0.0);
    format!(
        "({remaining_secs:.2} + {margin_secs:.2}) < ({total_playback_secs:.2} - {segment_secs:.2}) => ({adjusted_remaining:.2} < {playback_margin:.2}) = {}",
        adjusted_remaining < playback_margin
    )
}
