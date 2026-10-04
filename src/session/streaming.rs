use crate::tts::{StreamTimingProfile, VoiceError};
use std::time::Duration;
use tokio::sync::mpsc;

/// Start even if the prediction never reaches the normal startup threshold.
const MAX_STARTUP_BUFFERING: Duration = Duration::from_secs(60);
/// Recheck the time-based decision while waiting for the next chunk.
const DECISION_RECHECK_INTERVAL: Duration = Duration::from_millis(50);

pub(super) struct PlaybackStartGate {
    chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    timing: StreamTimingProfile,
    pending_chunks: Vec<Result<bytes::Bytes, VoiceError>>,
    received_bytes: u64,
    started: tokio::time::Instant,
}

pub(super) struct PlaybackStartGateOutput {
    pub(super) pending_chunks: Vec<Result<bytes::Bytes, VoiceError>>,
    pub(super) chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    pub(super) decision: PlaybackStartGateDecision,
}

pub(super) struct PlaybackStartGateDecision {
    pub(super) reason: PlaybackStartGateReleaseReason,
    pub(super) receive_playback_margin_secs: Option<f64>,
}

#[derive(Debug)]
pub(super) enum PlaybackStartGateReleaseReason {
    PredictionReady,
    MaxBufferingTime,
    StreamEnded,
    StreamError,
}

impl PlaybackStartGate {
    pub(super) fn new(
        chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
        timing: StreamTimingProfile,
    ) -> Self {
        Self {
            chunks,
            timing,
            pending_chunks: Vec::new(),
            received_bytes: 0,
            started: tokio::time::Instant::now(),
        }
    }

    pub(super) async fn run(mut self) -> PlaybackStartGateOutput {
        let max_deadline = self.started + MAX_STARTUP_BUFFERING;
        let reason;
        let mut receive_playback_margin_secs = None;
        loop {
            let now = tokio::time::Instant::now();
            if let Some((predicted, playable)) = self.startup_prediction(now) {
                let margin = playable - predicted;
                receive_playback_margin_secs = Some(margin);
                if margin > 0.0 {
                    reason = PlaybackStartGateReleaseReason::PredictionReady;
                    break;
                }
            }
            if now >= max_deadline {
                reason = PlaybackStartGateReleaseReason::MaxBufferingTime;
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
                        self.pending_chunks.push(chunk);
                        if is_error {
                            reason = PlaybackStartGateReleaseReason::StreamError;
                            break;
                        }
                    },
                    None => {
                        reason = PlaybackStartGateReleaseReason::StreamEnded;
                        break;
                    },
                },
                _ = tokio::time::sleep_until(poll_deadline) => {},
            }
        }
        PlaybackStartGateOutput {
            decision: PlaybackStartGateDecision {
                reason,
                receive_playback_margin_secs,
            },
            pending_chunks: self.pending_chunks,
            chunks: self.chunks,
        }
    }

    fn startup_prediction(&self, now: tokio::time::Instant) -> Option<(f64, f64)> {
        if !self.timing.has_received_audio_payload(self.received_bytes) {
            return None;
        }
        Some((
            self.timing
                .predicted_receive_completion_delay(now, self.received_bytes),
            self.timing.safe_playback_window().as_secs_f64(),
        ))
    }
}
