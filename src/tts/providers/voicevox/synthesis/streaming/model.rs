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
}

#[derive(Clone, Copy)]
pub(super) struct StreamingReceivePrediction {
    pub(super) total_receive_time: std::time::Duration,
    pub(super) receive_rate_secs_per_audio_sec: f64,
    pub(super) request_overhead_secs: f64,
    pub(super) segment_uncertainty: SegmentReceiveUncertainty,
}

pub(super) struct StreamingReceiveModelUpdate {
    pub(super) predicted_receive_time_secs: f64,
    pub(super) r_before: f64,
    pub(super) b_before: f64,
    pub(super) r_after: f64,
    pub(super) b_after: f64,
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

    pub(super) fn observe_completed_request(
        &mut self,
        audio_duration_secs: f64,
        receive_time_secs: f64,
    ) -> Option<StreamingReceiveModelUpdate> {
        if !audio_duration_secs.is_finite()
            || audio_duration_secs <= 0.0
            || !receive_time_secs.is_finite()
            || receive_time_secs <= 0.0
        {
            return None;
        }

        let features = Self::receive_time_features(audio_duration_secs);
        let predicted_receive_time_secs = self.rls.predict(features);
        let [r_before, b_before] = *self.rls.coefficients();
        self.rls.update(features, receive_time_secs).ok()?;
        let [r_after, b_after] = *self.rls.coefficients();

        Some(StreamingReceiveModelUpdate {
            predicted_receive_time_secs,
            r_before,
            b_before,
            r_after,
            b_after,
        })
    }

    fn segment_uncertainty(&self) -> SegmentReceiveUncertainty {
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
    }
}
