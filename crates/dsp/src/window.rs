use std::f32::consts::TAU;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Window {
    BlackmanHarris,
    Hann,
    Hamming,
    Rectangular,
    FlatTop,
}

impl Window {
    pub const ALL: [Window; 5] = [
        Window::BlackmanHarris,
        Window::Hann,
        Window::Hamming,
        Window::Rectangular,
        Window::FlatTop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Window::BlackmanHarris => "Blackman-Harris",
            Window::Hann => "Hann",
            Window::Hamming => "Hamming",
            Window::Rectangular => "Rectangular",
            Window::FlatTop => "Flat-top",
        }
    }

    /// Symmetric window of `length` taps, normalized so the peak is exactly 1.
    pub fn coefficients(self, length: usize) -> Vec<f32> {
        let cosine_terms: &[f32] = match self {
            Window::Rectangular => &[1.0],
            Window::Hann => &[0.5, -0.5],
            Window::Hamming => &[0.54, -0.46],
            Window::BlackmanHarris => &[0.35875, -0.48829, 0.14128, -0.01168],
            // Matlab flattopwin coefficients.
            Window::FlatTop => &[
                0.215_578_95,
                -0.416_631_58,
                0.277_263_16,
                -0.083_578_95,
                0.006947368,
            ],
        };
        let denominator = (length.max(2) - 1) as f32;
        let mut taps: Vec<f32> = (0..length)
            .map(|index| {
                let phase = TAU * index as f32 / denominator;
                cosine_terms
                    .iter()
                    .enumerate()
                    .map(|(order, term)| term * (order as f32 * phase).cos())
                    .sum::<f32>()
            })
            .collect();
        let peak = taps.iter().cloned().fold(f32::MIN, f32::max);
        for tap in &mut taps {
            *tap /= peak;
        }
        taps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_coefficients_peak_is_one() {
        for window in Window::ALL {
            let coefficients = window.coefficients(1024);
            assert_eq!(coefficients.len(), 1024, "{window:?}");
            let peak = coefficients.iter().cloned().fold(f32::MIN, f32::max);
            assert!((peak - 1.0).abs() < 1e-6, "{window:?} peak {peak}");
        }
    }
}
