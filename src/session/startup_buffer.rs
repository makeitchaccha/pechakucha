use crate::tts::{SegmentReceiveUncertainty, StreamTimingProfile, VoiceError};
use std::time::Duration;
use tokio::sync::mpsc;

const MAX_STARTUP_WAIT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const WAV_HEADER_BYTES: u64 = 44;
const PCM_BYTES_PER_SECOND: f64 = 48_000.0;
const COLD_START_MARGIN_SECS: f64 = 3.0;

pub(super) struct StartupBuffer {
    chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
    decider: Decider,
    buffered_chunks: Vec<Result<bytes::Bytes, VoiceError>>,
    received_bytes: u64,
    started: tokio::time::Instant,
}

#[derive(Clone, Copy)]
struct StartupTiming {
    request_started_at: tokio::time::Instant,
    estimated_total_receive_time: Duration,
    total_audio_playback_duration: Duration,
    chunk_audio_duration: Duration,
    receive_uncertainty: SegmentReceiveUncertainty,
    buffer_sigma: f64,
}

impl From<&StreamTimingProfile> for StartupTiming {
    fn from(profile: &StreamTimingProfile) -> Self {
        Self {
            request_started_at: profile.request_started_at,
            estimated_total_receive_time: profile.estimated_total_receive_time,
            total_audio_playback_duration: profile.total_audio_playback_duration,
            chunk_audio_duration: profile.chunk_audio_duration,
            receive_uncertainty: profile.receive_uncertainty,
            buffer_sigma: profile.buffer_sigma,
        }
    }
}

#[derive(Clone, Copy)]
struct Decider {
    timing: StartupTiming,
}

#[derive(Clone, Copy)]
struct Decision {
    timing: StartupTiming,
    received_pcm_bytes: u64,
    now: tokio::time::Instant,
}

impl Decider {
    fn new(timing: &StreamTimingProfile) -> Self {
        Self {
            timing: StartupTiming::from(timing),
        }
    }

    fn decide(&self, received_pcm_bytes: u64, now: tokio::time::Instant) -> Decision {
        Decision {
            timing: self.timing,
            received_pcm_bytes,
            now,
        }
    }
}

impl Decision {
    fn should_start(self) -> bool {
        if self.received_pcm_bytes == 0 {
            return false;
        }
        self.adjusted_remaining_receive_secs() < self.playback_margin().as_secs_f64()
    }

    fn remaining_receive_time(self) -> Duration {
        self.timing.estimated_total_receive_time.saturating_sub(
            self.now
                .saturating_duration_since(self.timing.request_started_at),
        )
    }

    fn adjusted_remaining_receive_secs(self) -> f64 {
        self.remaining_receive_time().as_secs_f64() + self.receive_margin_secs()
    }

    fn playback_margin(self) -> Duration {
        self.timing
            .total_audio_playback_duration
            .saturating_sub(self.timing.chunk_audio_duration)
    }

    fn receive_margin_secs(self) -> f64 {
        self.receive_margin_secs_for(self.received_pcm_bytes)
    }

    fn receive_margin_secs_for(self, pcm_bytes: u64) -> f64 {
        match self.timing.receive_uncertainty {
            SegmentReceiveUncertainty::Warmup => COLD_START_MARGIN_SECS,
            SegmentReceiveUncertainty::Estimated {
                standard_deviation_secs,
            } => {
                self.timing.buffer_sigma
                    * standard_deviation_secs
                    * self.remaining_segment_count(pcm_bytes).sqrt()
            }
        }
    }

    fn remaining_segment_count(self, pcm_bytes: u64) -> f64 {
        let segment_secs = self.timing.chunk_audio_duration.as_secs_f64();
        if segment_secs <= 0.0 {
            return 0.0;
        }
        let received_audio_secs = pcm_bytes as f64 / PCM_BYTES_PER_SECOND;
        ((self.timing.total_audio_playback_duration.as_secs_f64() - received_audio_secs).max(0.0)
            / segment_secs)
            .ceil()
    }
}

impl StartupBuffer {
    pub(super) fn new(
        chunks: mpsc::Receiver<Result<bytes::Bytes, VoiceError>>,
        timing: StreamTimingProfile,
    ) -> Self {
        Self {
            chunks,
            decider: Decider::new(&timing),
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
        let max_deadline = self.started + MAX_STARTUP_WAIT;
        loop {
            let now = tokio::time::Instant::now();
            let pcm_bytes = self.received_bytes.saturating_sub(WAV_HEADER_BYTES);
            let decision = self.decider.decide(pcm_bytes, now);
            if decision.should_start() {
                break;
            }
            if now >= max_deadline {
                break;
            }
            let poll_deadline = (now + POLL_INTERVAL).min(max_deadline);
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
}
