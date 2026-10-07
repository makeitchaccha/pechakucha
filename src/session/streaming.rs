use crate::tts::{StreamTimingProfile, VoiceError};
use std::time::Duration;
use tokio::sync::mpsc;

/// Start even if the prediction never reaches the normal startup threshold.
const MAX_STARTUP_BUFFERING: Duration = Duration::from_secs(60);

pub(super) struct PlaybackPrebuffer {
    pub(super) pending_chunks: Vec<Result<bytes::Bytes, VoiceError>>,
    pub(super) chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    pub(super) decision: PlaybackPrebufferDecision,
}

pub(super) struct PlaybackPrebufferDecision {
    pub(super) reason: PlaybackPrebufferReleaseReason,
    pub(super) receive_playback_margin_secs: Option<f64>,
}

#[derive(Debug)]
pub(super) enum PlaybackPrebufferReleaseReason {
    PredictionReady,
    MaxBufferingTime,
    StreamEnded,
    StreamError,
}

pub(super) async fn prebuffer_for_playback(
    mut chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    timing: StreamTimingProfile,
) -> PlaybackPrebuffer {
    let started = tokio::time::Instant::now();
    let max_deadline = started + MAX_STARTUP_BUFFERING;
    let mut pending_chunks = Vec::new();
    let mut received_bytes = 0u64;
    let decision = loop {
        let now = tokio::time::Instant::now();
        let margin = startup_margin(&timing, received_bytes, now);
        if let Some(margin) = margin {
            if margin > 0.0 {
                break PlaybackPrebufferDecision {
                    reason: PlaybackPrebufferReleaseReason::PredictionReady,
                    receive_playback_margin_secs: Some(margin),
                };
            }
        }
        if now >= max_deadline {
            break PlaybackPrebufferDecision {
                reason: PlaybackPrebufferReleaseReason::MaxBufferingTime,
                receive_playback_margin_secs: margin,
            };
        }
        let deadline = next_deadline(&timing, now, max_deadline, margin);
        tokio::select! {
            biased;
            chunk = chunks.recv() => match chunk {
                Some(chunk) => {
                    if let Ok(bytes) = &chunk {
                        received_bytes = received_bytes.saturating_add(bytes.len() as u64);
                    }
                    let is_error = chunk.is_err();
                    pending_chunks.push(chunk);
                    if is_error {
                        break PlaybackPrebufferDecision {
                            reason: PlaybackPrebufferReleaseReason::StreamError,
                            receive_playback_margin_secs: margin,
                        };
                    }
                },
                None => {
                    break PlaybackPrebufferDecision {
                        reason: PlaybackPrebufferReleaseReason::StreamEnded,
                        receive_playback_margin_secs: margin,
                    };
                },
            },
            _ = tokio::time::sleep_until(deadline) => {},
        }
    };
    PlaybackPrebuffer {
        decision,
        pending_chunks,
        chunks,
    }
}

fn startup_margin(
    timing: &StreamTimingProfile,
    received_bytes: u64,
    now: tokio::time::Instant,
) -> Option<f64> {
    if !timing.has_received_audio_payload(received_bytes) {
        return None;
    }
    Some(
        timing.safe_playback_window().as_secs_f64()
            - timing.predicted_receive_completion_delay(now, received_bytes),
    )
}

fn next_deadline(
    timing: &StreamTimingProfile,
    now: tokio::time::Instant,
    max_deadline: tokio::time::Instant,
    margin: Option<f64>,
) -> tokio::time::Instant {
    match margin {
        Some(margin) if now < timing.request_started_at + timing.estimated_total_receive_time => {
            let until_ready = Duration::from_secs_f64((-margin).max(0.0)) + Duration::from_nanos(1);
            (now + until_ready).min(max_deadline)
        }
        _ => max_deadline,
    }
}
