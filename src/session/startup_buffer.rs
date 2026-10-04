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

    pub(super) async fn run(
        mut self,
    ) -> (
        Vec<Result<bytes::Bytes, VoiceError>>,
        mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    ) {
        let max_deadline = self.started + MAX_STARTUP_BUFFERING;
        loop {
            let now = tokio::time::Instant::now();
            if self.should_start(now) {
                break;
            }
            if now >= max_deadline {
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
                            break;
                        }
                    },
                    None => break,
                },
                _ = tokio::time::sleep_until(poll_deadline) => {},
            }
        }
        (self.buffered_chunks, self.chunks)
    }

    fn should_start(&self, now: tokio::time::Instant) -> bool {
        self.timing.has_received_audio_payload(self.received_bytes)
            && self
                .timing
                .predicted_remaining_receive_time(now, self.received_bytes)
                < self.timing.playback_time_after_start().as_secs_f64()
    }
}
