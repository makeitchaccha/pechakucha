use thiserror::Error;

/// Recursive least-squares regression for a fixed-size feature vector.
///
/// The caller supplies the initial coefficients `θ₀`, a symmetric
/// positive-definite initial covariance matrix `P₀`, and a forgetting factor
/// in `(0, 1]`.
/// For each observation, the model applies `K = P*x / (lambda + xᵀ*P*x)`,
/// updates the coefficients by `K * (y - prediction)`, and updates the
/// covariance matrix with the forgetting factor.
pub struct RecursiveLeastSquares<const N: usize> {
    /// Current coefficient state `θₜ`, updated after every observation.
    coefficients: [f64; N],
    /// Current covariance state `Pₜ`, updated after every observation.
    covariance: [[f64; N]; N],
    forgetting_factor: f64,
}

#[derive(Debug, Error)]
pub enum RlsError {
    #[error("RLS features and observation must be finite")]
    NonFiniteObservation,
    #[error("RLS update is numerically unstable")]
    NumericalInstability,
}

impl<const N: usize> RecursiveLeastSquares<N> {
    pub fn new(
        initial_coefficients: [f64; N],
        initial_covariance: [[f64; N]; N],
        forgetting_factor: f64,
    ) -> Self {
        Self {
            coefficients: initial_coefficients,
            covariance: initial_covariance,
            forgetting_factor,
        }
    }

    /// Returns the current coefficient state `θₜ`.
    pub fn coefficients(&self) -> &[f64; N] {
        &self.coefficients
    }

    pub fn predict(&self, features: [f64; N]) -> f64 {
        dot(self.coefficients, features)
    }

    pub fn update(&mut self, features: [f64; N], observation: f64) -> Result<(), RlsError> {
        if !observation.is_finite() || !features.iter().all(|feature| feature.is_finite()) {
            return Err(RlsError::NonFiniteObservation);
        }

        let old_covariance = self.covariance;
        let predicted = self.predict(features);
        let error = observation - predicted;
        let projected_covariance: [f64; N] = std::array::from_fn(|row| {
            (0..N)
                .map(|column| old_covariance[row][column] * features[column])
                .sum()
        });
        let denominator = self.forgetting_factor + dot(features, projected_covariance);
        if !denominator.is_finite() || denominator <= 0.0 {
            return Err(RlsError::NumericalInstability);
        }

        let gain: [f64; N] = std::array::from_fn(|index| projected_covariance[index] / denominator);
        let next_coefficients: [f64; N] =
            std::array::from_fn(|index| self.coefficients[index] + gain[index] * error);
        let next_covariance: [[f64; N]; N] = std::array::from_fn(|row| {
            std::array::from_fn(|column| {
                (old_covariance[row][column] - gain[row] * projected_covariance[column])
                    / self.forgetting_factor
            })
        });

        if !next_coefficients.iter().all(|value| value.is_finite())
            || !next_covariance
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        {
            return Err(RlsError::NumericalInstability);
        }

        self.coefficients = next_coefficients;
        self.covariance = next_covariance;
        Ok(())
    }
}

fn dot<const N: usize>(left: [f64; N], right: [f64; N]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}
