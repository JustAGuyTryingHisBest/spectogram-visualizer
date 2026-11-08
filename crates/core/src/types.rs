use std::sync::Arc;
use std::time::Instant;

use num_complex::Complex32;
use serde::{Deserialize, Serialize};

/// Metadata every source stamps on what it emits.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    pub name: String,
    pub center_hz: f64,
    pub sample_rate_hz: f64,
    /// Added to every dBFS bin to produce dBm, when the source knows its calibration.
    pub calibration_offset_db: Option<f32>,
}

/// A block of complex baseband samples from an IQ source.
#[derive(Clone, Debug)]
pub struct IqBlock {
    pub info: StreamInfo,
    pub samples: Arc<[Complex32]>,
    pub sequence: u64,
    pub received_at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PowerUnits {
    Dbfs,
    Dbm,
}

/// One finished power-versus-frequency row in FFT-shifted order (DC at index `len / 2`).
#[derive(Clone, Debug)]
pub struct SpectrumRow {
    pub center_hz: f64,
    pub span_hz: f64,
    pub units: PowerUnits,
    pub power_db: Arc<[f32]>,
    pub received_at: Instant,
}

impl SpectrumRow {
    pub fn bin_width_hz(&self) -> f64 {
        self.span_hz / self.power_db.len() as f64
    }

    pub fn bin_hz(&self, bin_index: usize) -> f64 {
        let half_bins = self.power_db.len() as f64 / 2.0;
        self.center_hz + (bin_index as f64 - half_bins) * self.bin_width_hz()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_hz_centers_span() {
        let bin_count = 2048;
        let row = SpectrumRow {
            center_hz: 915e6,
            span_hz: 20e6,
            units: PowerUnits::Dbfs,
            power_db: Arc::from(vec![0.0f32; bin_count]),
            received_at: Instant::now(),
        };
        assert_eq!(row.bin_width_hz(), 20e6 / 2048.0);
        assert_eq!(row.bin_hz(1024), 915e6);
    }
}
