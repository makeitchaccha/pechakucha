use crate::tts::{StreamTimingProfile, VoiceError};
use std::time::Duration;
use tokio::sync::mpsc;

/// Start even if the prediction never reaches the normal startup threshold.
const MAX_STARTUP_BUFFERING: Duration = Duration::from_secs(60);
/// Recheck the time-based decision while waiting for the next chunk.
const DECISION_RECHECK_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct StartupBuffer {
    chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    timing: StreamTimingProfile,
    buffered_chunks: Vec<Result<bytes::Bytes, VoiceError>>,
    received_bytes: u64,
    started: tokio::time::Instant,
}

pub(super) struct StartupBufferOutput {
    pub(super) buffered_chunks: Vec<Result<bytes::Bytes, VoiceError>>,
    pub(super) chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    pub(super) decision: StartupBufferDecision,
}

pub(super) struct StartupBufferDecision {
    pub(super) reason: StartupBufferReleaseReason,
    pub(super) receive_playback_margin_secs: Option<f64>,
}

#[derive(Debug)]
pub(super) enum StartupBufferReleaseReason {
    PredictionReady,
    MaxBufferingTime,
    StreamEnded,
    StreamError,
}

impl StartupBuffer {
    pub(super) fn new(
        chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
        timing: StreamTimingProfile,
    ) -> Self {
        Self {
            chunks,
            timing,
            buffered_chunks: Vec::new(),
            received_bytes: 0,
            started: tokio::time::Instant::now(),
        }
    }

    pub(super) async fn run(mut self) -> StartupBufferOutput {
        let max_deadline = self.started + MAX_STARTUP_BUFFERING;
        let reason;
        let mut receive_playback_margin_secs = None;
        loop {
            let now = tokio::time::Instant::now();
            if let Some((predicted, playable)) = self.startup_prediction(now) {
                let margin = playable - predicted;
                receive_playback_margin_secs = Some(margin);
                if margin > 0.0 {
                    reason = StartupBufferReleaseReason::PredictionReady;
                    break;
                }
            }
            if now >= max_deadline {
                reason = StartupBufferReleaseReason::MaxBufferingTime;
                break;
            }
            let poll_deadline = (now + DECISION_RECHECK_INTERVAL).min(max_deadline);
            tokio::select! {
                biased;
                chunk = self.chunks.recv() => match chunk {
                    Some(chunk) => {
                        if let Ok(bytes) = &chunk {
                            self.received_bytes = self.received_bytes.saturating_add(bytes.len() as u64);
                        }
                        let is_error = chunk.is_err();
                        self.buffered_chunks.push(chunk);
                        if is_error {
                            reason = StartupBufferReleaseReason::StreamError;
                            break;
                        }
                    },
                    None => {
                        reason = StartupBufferReleaseReason::StreamEnded;
                        break;
                    },
                },
                _ = tokio::time::sleep_until(poll_deadline) => {},
            }
        }
        StartupBufferOutput {
            decision: StartupBufferDecision {
                reason,
                receive_playback_margin_secs,
            },
            buffered_chunks: self.buffered_chunks,
            chunks: self.chunks,
        }
    }

    fn startup_prediction(&self, now: tokio::time::Instant) -> Option<(f64, f64)> {
        if !self.timing.has_received_audio_payload(self.received_bytes) {
            return None;
        }
        Some((
            self.timing
                .predicted_remaining_receive_time(now, self.received_bytes),
            self.timing.playback_time_after_start().as_secs_f64(),
        ))
    }
}
