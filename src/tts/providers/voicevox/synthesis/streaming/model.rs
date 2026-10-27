use crate::math::recursive_least_squares::RecursiveLeastSquares;
use crate::tts::{SegmentReceiveUncertainty, VoiceError};

struct SegmentUncertaintyModel {
    error_variance_secs2: f64,
    samples_seen: u64,
    alpha: f64,
    min_samples: u64,
}

impl SegmentUncertaintyModel {
    fn new(alpha: f64, min_samples: u64) -> Self {
        Self {
            error_variance_secs2: 0.0,
            samples_seen: 0,
            alpha,
            min_samples,
        }
    }

    fn observe_squared_error(&mut self, squared_error: f64) {
        self.error_variance_secs2 =
            (1.0 - self.alpha) * self.error_variance_secs2 + self.alpha * squared_error;
        self.samples_seen = self.samples_seen.saturating_add(1);
    }

    fn standard_deviation_secs(&self) -> Option<f64> {
        (self.samples_seen >= self.min_samples).then(|| self.error_variance_secs2.max(0.0).sqrt())
    }
}

/// RLS estimates for request receive time and EMA uncertainty for
/// segment-to-segment receive-time variation.
pub(super) struct StreamingReceiveModel {
    rls: RecursiveLeastSquares<2>,
    segment_uncertainty: SegmentUncertaintyModel,
    current_cumulative_audio_secs: f64,
    current_cumulative_receive_secs: f64,
}

#[derive(Clone, Copy)]
pub(super) struct StreamingReceivePrediction {
    pub(super) total_receive_time: std::time::Duration,
    pub(super) receive_rate_secs_per_audio_sec: f64,
    pub(super) request_overhead_secs: f64,
    pub(super) segment_uncertainty: SegmentReceiveUncertainty,
}

impl StreamingReceiveModel {
    const INITIAL_COEFFICIENTS: [f64; 2] = [1.0, 0.0];
    const INITIAL_COVARIANCE: [[f64; 2]; 2] = [[100.0, 0.0], [0.0, 100.0]];
    const FORGETTING_FACTOR: f64 = 0.9;
    /// Ignore short chunks when measuring the receive interval of full segments.
    pub(super) const MIN_SEGMENT_CHUNK_COMPLETENESS: f64 = 0.9;
    const UNCERTAINTY_ALPHA: f64 = 0.05;
    const MIN_UNCERTAINTY_SAMPLES: u64 = 5;

    pub(super) fn new() -> Self {
        Self {
            rls: RecursiveLeastSquares::new(
                Self::INITIAL_COEFFICIENTS,
                Self::INITIAL_COVARIANCE,
                Self::FORGETTING_FACTOR,
            ),
            segment_uncertainty: SegmentUncertaintyModel::new(
                Self::UNCERTAINTY_ALPHA,
                Self::MIN_UNCERTAINTY_SAMPLES,
            ),
            current_cumulative_audio_secs: 0.0,
            current_cumulative_receive_secs: 0.0,
        }
    }

    pub(super) fn predict(
        &self,
        audio_duration: std::time::Duration,
    ) -> Result<StreamingReceivePrediction, VoiceError> {
        let [receive_rate_secs_per_audio_sec, request_overhead_secs] = *self.rls.coefficients();
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
            receive_rate_secs_per_audio_sec,
            request_overhead_secs,
            segment_uncertainty: self.segment_uncertainty(),
        })
    }

    fn receive_time_features(audio_duration_secs: f64) -> [f64; 2] {
        [audio_duration_secs, 1.0]
    }

    pub(super) fn segment_uncertainty(&self) -> SegmentReceiveUncertainty {
        match self.segment_uncertainty.standard_deviation_secs() {
            Some(standard_deviation_secs) => SegmentReceiveUncertainty::Estimated {
                standard_deviation_secs,
            },
            None => SegmentReceiveUncertainty::Warmup,
        }
    }

    pub(super) fn observe_segment_interval(&mut self, interval_secs: f64, audio_segment_secs: f64) {
        if !interval_secs.is_finite()
            || interval_secs <= 0.0
            || !audio_segment_secs.is_finite()
            || audio_segment_secs <= 0.0
        {
            return;
        }

        let expected_interval = self.rls.coefficients()[0] * audio_segment_secs;
        let segment_error = interval_secs - expected_interval;
        self.segment_uncertainty
            .observe_squared_error(segment_error * segment_error);

        // Current-request prediction state: accumulate observed segment
        // receive time vs. audio so the final receive time can be
        // re-estimated as segments arrive. These cumulative values are NOT
        // fed into the request-level RLS learner (they are correlated).
        self.current_cumulative_audio_secs += audio_segment_secs;
        self.current_cumulative_receive_secs += interval_secs;
    }

    /// Re-estimate the final receive time for the current request using the
    /// segments observed so far. Returns `None` until at least one full
    /// segment has been observed, in which case the caller keeps the
    /// (request-level RLS) prediction made at request start.
    pub(super) fn current_request_predicted_total_secs(&self, total_audio_secs: f64) -> Option<f64> {
        if self.current_cumulative_audio_secs <= 0.0 || !total_audio_secs.is_finite() || total_audio_secs <= 0.0 {
            return None;
        }
        let observed_rate = self.current_cumulative_receive_secs / self.current_cumulative_audio_secs;
        // `b` is request-level overhead (from RLS); segments exclude it.
        let overhead = self.rls.coefficients()[1];
        Some((observed_rate * total_audio_secs + overhead).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_request_prediction_changes_as_segments_arrive() {
        let mut model = StreamingReceiveModel::new();
        let total_audio = 10.0;
        assert!(model.current_request_predicted_total_secs(total_audio).is_none());

        model.observe_segment_interval(3.0, 2.0);
        let p1 = model
            .current_request_predicted_total_secs(total_audio)
            .expect("prediction after first segment");
        assert!((p1 - 15.0).abs() < 1e-9, "p1={p1}");

        model.observe_segment_interval(1.0, 2.0);
        let p2 = model
            .current_request_predicted_total_secs(total_audio)
            .expect("prediction after second segment");
        assert!((p2 - 10.0).abs() < 1e-9, "p2={p2}");

        assert!((p1 - p2).abs() > 1e-9);
    }
}
