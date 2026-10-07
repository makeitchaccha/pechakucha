use crate::tts::{StreamTimingProfile, VoiceError};
use std::time::Duration;
use tokio::sync::mpsc;

/// Start even if the prediction never reaches the normal startup threshold.
const MAX_STARTUP_BUFFERING: Duration = Duration::from_secs(60);

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
            let margin = self
                .startup_prediction(now)
                .map(|(predicted, playable)| playable - predicted);
            if let Some(margin) = margin {
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
            let deadline = self.next_deadline(now, max_deadline, margin);
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
                _ = tokio::time::sleep_until(deadline) => {},
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

    fn next_deadline(
        &self,
        now: tokio::time::Instant,
        max_deadline: tokio::time::Instant,
        margin: Option<f64>,
    ) -> tokio::time::Instant {
        match margin {
            Some(margin)
                if now
                    < self.timing.request_started_at + self.timing.estimated_total_receive_time =>
            {
                let until_ready =
                    Duration::from_secs_f64((-margin).max(0.0)) + Duration::from_nanos(1);
                (now + until_ready).min(max_deadline)
            }
            _ => max_deadline,
        }
    }
}
