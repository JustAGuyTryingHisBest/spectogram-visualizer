use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};

use crate::window::Window;

/// Power floor in dB for bins with zero energy.
pub const POWER_FLOOR_DB: f32 = -200.0;

/// A slice of the source capture to view, selected by digital down-conversion.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct SubBand {
    pub offset_hz: f64,
    pub decimation: u32,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ChainConfig {
    pub fft_size: usize,
    pub window: Window,
    pub averaging: usize,
    pub max_hold: bool,
    pub sub_band: Option<SubBand>,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            fft_size: 2048,
            window: Window::BlackmanHarris,
            averaging: 8,
            max_hold: true,
            sub_band: None,
        }
    }
}

pub struct ChainOutput {
    pub live_db: Arc<[f32]>,
    pub average_db: Arc<[f32]>,
    pub max_hold_db: Option<Arc<[f32]>>,
}

/// Gapless window + FFT + dB + averaging + max-hold. Every pushed sample is consumed.
pub struct Chain {
    config: ChainConfig,
    window_taps: Vec<f32>,
    power_scale: f32,
    fft: Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,
    frame: Vec<Complex32>,
    carry: Vec<Complex32>,
    traces: TraceAccumulator,
}

impl Chain {
    pub fn new(config: ChainConfig) -> Self {
        let fft_size = config.fft_size.max(2);
        let window_taps = config.window.coefficients(fft_size);
        let coherent_gain: f32 = window_taps.iter().sum();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(fft_size);
        Self {
            fft_scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            window_taps,
            // Full-scale tone through this window reads 0 dBFS.
            power_scale: 1.0 / (coherent_gain * coherent_gain),
            frame: vec![Complex32::default(); fft_size],
            carry: Vec::with_capacity(fft_size),
            traces: TraceAccumulator::new(config.averaging, config.max_hold),
            config,
        }
    }

    pub fn config(&self) -> &ChainConfig {
        &self.config
    }

    pub fn reset_max_hold(&mut self) {
        self.traces.reset_max_hold();
    }

    /// Consumes every sample; calls `emit` once per completed FFT frame, FFT-shifted (DC at n/2).
    pub fn push(&mut self, samples: &[Complex32], emit: &mut dyn FnMut(ChainOutput)) {
        let fft_size = self.window_taps.len();
        let mut remaining = samples;
        while !remaining.is_empty() {
            let needed = fft_size - self.carry.len();
            let take = needed.min(remaining.len());
            self.carry.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.carry.len() == fft_size {
                self.transform_carry();
                let live_db = self.live_db_shifted();
                let (average_db, max_hold_db) = self.traces.push(&live_db);
                emit(ChainOutput {
                    live_db,
                    average_db,
                    max_hold_db,
                });
                self.carry.clear();
            }
        }
    }

    fn transform_carry(&mut self) {
        for ((slot, sample), tap) in self
            .frame
            .iter_mut()
            .zip(&self.carry)
            .zip(&self.window_taps)
        {
            *slot = sample * tap;
        }
        self.fft
            .process_with_scratch(&mut self.frame, &mut self.fft_scratch);
    }

    fn live_db_shifted(&self) -> Arc<[f32]> {
        let fft_size = self.frame.len();
        let half = fft_size / 2;
        let mut live_db = Vec::with_capacity(fft_size);
        for index in 0..fft_size {
            let bin = self.frame[(index + half) % fft_size];
            let power = bin.norm_sqr() * self.power_scale;
            live_db.push(if power > 0.0 {
                (10.0 * power.log10()).max(POWER_FLOOR_DB)
            } else {
                POWER_FLOOR_DB
            });
        }
        Arc::from(live_db)
    }
}

/// Exponential average and max-hold over successive rows. Shared by `Chain` and spectrum-source views.
pub struct TraceAccumulator {
    averaging: usize,
    max_hold_enabled: bool,
    average_db: Vec<f32>,
    max_hold_db: Vec<f32>,
    reset_max_hold_pending: bool,
}

impl TraceAccumulator {
    pub fn new(averaging: usize, max_hold_enabled: bool) -> Self {
        Self {
            averaging: averaging.max(1),
            max_hold_enabled,
            average_db: Vec::new(),
            max_hold_db: Vec::new(),
            reset_max_hold_pending: true,
        }
    }

    pub fn reset_max_hold(&mut self) {
        self.reset_max_hold_pending = true;
    }

    pub fn push(&mut self, live_db: &[f32]) -> (Arc<[f32]>, Option<Arc<[f32]>>) {
        if self.average_db.len() != live_db.len() {
            self.average_db = live_db.to_vec();
            self.reset_max_hold_pending = true;
        } else {
            let weight = 1.0 / self.averaging as f32;
            for (average, live) in self.average_db.iter_mut().zip(live_db) {
                *average += (live - *average) * weight;
            }
        }
        let max_hold = if self.max_hold_enabled {
            if self.reset_max_hold_pending {
                self.max_hold_db = live_db.to_vec();
                self.reset_max_hold_pending = false;
            } else {
                for (held, live) in self.max_hold_db.iter_mut().zip(live_db) {
                    *held = held.max(*live);
                }
            }
            Some(Arc::from(self.max_hold_db.as_slice()))
        } else {
            None
        };
        (Arc::from(self.average_db.as_slice()), max_hold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex32;
    use std::f32::consts::TAU;

    fn tone(sample_count: usize, cycles_per_sample: f32) -> Vec<Complex32> {
        (0..sample_count)
            .map(|index| Complex32::from_polar(1.0, TAU * cycles_per_sample * index as f32))
            .collect()
    }

    fn collect_outputs(chain: &mut Chain, samples: &[Complex32]) -> Vec<ChainOutput> {
        let mut outputs = Vec::new();
        chain.push(samples, &mut |output| outputs.push(output));
        outputs
    }

    fn argmax(values: &[f32]) -> usize {
        values
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap()
    }

    #[test]
    fn tone_lands_in_expected_bin() {
        let mut chain = Chain::new(ChainConfig {
            fft_size: 1024,
            window: Window::Rectangular,
            averaging: 1,
            max_hold: false,
            sub_band: None,
        });
        let outputs = collect_outputs(&mut chain, &tone(4096, 1.0 / 8.0));
        assert_eq!(outputs.len(), 4);
        for output in &outputs {
            let peak_bin = argmax(&output.live_db);
            assert_eq!(peak_bin, 512 + 128);
            assert!(
                output.live_db[peak_bin].abs() < 0.5,
                "{}",
                output.live_db[peak_bin]
            );
        }
    }

    #[test]
    fn chain_is_gapless() {
        let mut chain = Chain::new(ChainConfig {
            fft_size: 1024,
            ..ChainConfig::default()
        });
        let samples = tone(2048, 0.01);
        let first = collect_outputs(&mut chain, &samples[..1000]);
        let second = collect_outputs(&mut chain, &samples[1000..]);
        assert_eq!(first.len() + second.len(), 2);
    }

    #[test]
    fn average_and_max_hold_behave() {
        let mut chain = Chain::new(ChainConfig {
            fft_size: 256,
            window: Window::Rectangular,
            averaging: 4,
            max_hold: true,
            sub_band: None,
        });
        let tone_outputs = collect_outputs(&mut chain, &tone(256, 1.0 / 8.0));
        let peak_bin = argmax(&tone_outputs[0].live_db);
        let tone_peak_db = tone_outputs[0].live_db[peak_bin];

        let silence = vec![Complex32::new(0.0, 0.0); 256 * 8];
        let silent_outputs = collect_outputs(&mut chain, &silence);
        let last = silent_outputs.last().unwrap();
        assert!(last.live_db[peak_bin] <= -200.0 + 1e-3);
        assert!((last.max_hold_db.as_ref().unwrap()[peak_bin] - tone_peak_db).abs() < 1e-3);
        assert!(last.average_db[peak_bin] < tone_peak_db - 20.0);
        assert!(last.average_db[peak_bin] > -200.0);

        chain.reset_max_hold();
        let after_reset = collect_outputs(&mut chain, &silence[..256]);
        let output = &after_reset[0];
        assert_eq!(
            &output.max_hold_db.as_ref().unwrap()[..],
            &output.live_db[..]
        );
    }
}
