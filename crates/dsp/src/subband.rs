use rustfft::num_complex::{Complex32, Complex64};

use crate::chain::SubBand;

/// Digital down-converter: mixes a sub-band to baseband, low-pass filters, and decimates.
/// Phase and filter state carry across `process` calls so a split stream equals a whole one.
pub struct Ddc {
    sub_band: SubBand,
    input_rate_hz: f64,
    /// Current NCO phasor, advanced by multiplication and renormalized periodically.
    phasor: Complex64,
    phasor_step: Complex64,
    samples_since_renormalize: u32,
    taps: Vec<f32>,
    /// Mixed samples not yet consumed by the filter; the first `taps.len() - 1` are history.
    pending: Vec<Complex32>,
}

/// Taps per unit of decimation: 32D+1 Blackman-windowed sinc taps hold the passband flat to
/// 0.9 × the output Nyquist and reject anything that would alias into it by more than 70 dB.
const TAPS_PER_DECIMATION: usize = 32;
const RENORMALIZE_EVERY: u32 = 1024;

impl Ddc {
    pub fn new(sub_band: SubBand, input_rate_hz: f64) -> Self {
        let decimation = sub_band.decimation.max(1) as usize;
        let taps = low_pass_taps(
            TAPS_PER_DECIMATION * decimation + 1,
            0.5 / decimation as f64,
        );
        let phase_step = -std::f64::consts::TAU * sub_band.offset_hz / input_rate_hz;
        Self {
            sub_band,
            input_rate_hz,
            phasor: Complex64::new(1.0, 0.0),
            phasor_step: Complex64::from_polar(1.0, phase_step),
            samples_since_renormalize: 0,
            pending: vec![Complex32::default(); taps.len() - 1],
            taps,
        }
    }

    pub fn output_rate_hz(&self) -> f64 {
        self.input_rate_hz / f64::from(self.sub_band.decimation.max(1))
    }

    /// Appends decimated baseband samples to `out`.
    pub fn process(&mut self, input: &[Complex32], out: &mut Vec<Complex32>) {
        for sample in input {
            let mixer = Complex32::new(self.phasor.re as f32, self.phasor.im as f32);
            self.pending.push(sample * mixer);
            self.phasor *= self.phasor_step;
            self.samples_since_renormalize += 1;
            if self.samples_since_renormalize == RENORMALIZE_EVERY {
                self.phasor /= self.phasor.norm();
                self.samples_since_renormalize = 0;
            }
        }
        let tap_count = self.taps.len();
        let decimation = self.sub_band.decimation.max(1) as usize;
        let mut read_index = 0;
        while read_index + tap_count <= self.pending.len() {
            let window = &self.pending[read_index..read_index + tap_count];
            let mut accumulator = Complex32::default();
            for (sample, tap) in window.iter().zip(&self.taps) {
                accumulator += sample * tap;
            }
            out.push(accumulator);
            read_index += decimation;
        }
        self.pending.drain(..read_index);
    }
}

/// Blackman-windowed sinc low-pass with `cutoff` as a fraction of the input sample rate, unity DC gain.
fn low_pass_taps(tap_count: usize, cutoff: f64) -> Vec<f32> {
    let center = (tap_count - 1) as f64 / 2.0;
    let mut taps: Vec<f64> = (0..tap_count)
        .map(|index| {
            let offset = index as f64 - center;
            let sinc = if offset == 0.0 {
                2.0 * cutoff
            } else {
                (std::f64::consts::TAU * cutoff * offset).sin() / (std::f64::consts::PI * offset)
            };
            let window_phase = std::f64::consts::TAU * index as f64 / (tap_count - 1) as f64;
            let blackman = 0.42 - 0.5 * window_phase.cos() + 0.08 * (2.0 * window_phase).cos();
            sinc * blackman
        })
        .collect();
    let gain: f64 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= gain;
    }
    taps.into_iter().map(|tap| tap as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::FftPlanner;
    use std::f32::consts::TAU;

    fn tone(sample_count: usize, frequency_hz: f64, sample_rate_hz: f64) -> Vec<Complex32> {
        (0..sample_count)
            .map(|index| {
                let phase = TAU * (frequency_hz / sample_rate_hz) as f32 * index as f32;
                Complex32::from_polar(1.0, phase)
            })
            .collect()
    }

    /// Power in dB of the strongest FFT bin and that bin's index (FFT-shifted), over `fft_size` samples.
    fn peak(samples: &[Complex32], fft_size: usize) -> (usize, f32) {
        let mut frame = samples[samples.len() - fft_size..].to_vec();
        FftPlanner::<f32>::new()
            .plan_fft_forward(fft_size)
            .process(&mut frame);
        let half = fft_size / 2;
        (0..fft_size)
            .map(|index| {
                let bin = frame[(index + half) % fft_size];
                (
                    index,
                    10.0 * (bin.norm_sqr() / (fft_size * fft_size) as f32).log10(),
                )
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap()
    }

    #[test]
    fn ddc_moves_tone_to_baseband() {
        let input_rate = 2e6;
        let input = tone(8192, 250e3, input_rate);
        let mut ddc = Ddc::new(
            SubBand {
                offset_hz: 250e3,
                decimation: 4,
            },
            input_rate,
        );
        assert_eq!(ddc.output_rate_hz(), 500e3);
        let mut output = Vec::new();
        ddc.process(&input, &mut output);
        let expected_len = input.len() / 4;
        assert!(
            (output.len() as i64 - expected_len as i64).abs() <= 64,
            "got {} samples, expected about {expected_len}",
            output.len()
        );
        let (bin, power_db) = peak(&output, 1024);
        assert!((bin as i64 - 512).abs() <= 1, "peak bin {bin}");
        assert!(power_db > -1.0, "in-band tone attenuated to {power_db} dB");
    }

    #[test]
    fn ddc_rejects_out_of_band_tone() {
        let input_rate = 2e6;
        let sub_band = SubBand {
            offset_hz: 250e3,
            decimation: 4,
        };
        let mut in_band = Vec::new();
        Ddc::new(sub_band, input_rate).process(&tone(8192, 250e3, input_rate), &mut in_band);
        let mut out_of_band = Vec::new();
        Ddc::new(sub_band, input_rate).process(&tone(8192, 900e3, input_rate), &mut out_of_band);
        let (_, in_band_db) = peak(&in_band, 1024);
        let (_, out_of_band_db) = peak(&out_of_band, 1024);
        assert!(
            in_band_db - out_of_band_db >= 40.0,
            "in {in_band_db} dB, out {out_of_band_db} dB"
        );
    }

    #[test]
    fn ddc_rejects_tone_that_would_alias_into_the_view() {
        // Output rate 500 kS/s; a tone at 0.65 × output Nyquist beyond the edge folds into the view.
        let input_rate = 2e6;
        let sub_band = SubBand {
            offset_hz: 0.0,
            decimation: 4,
        };
        let mut in_band = Vec::new();
        Ddc::new(sub_band, input_rate)
            .process(&tone(16384, 0.38 * 250e3, input_rate), &mut in_band);
        let mut aliasing = Vec::new();
        Ddc::new(sub_band, input_rate)
            .process(&tone(16384, 0.65 * 500e3, input_rate), &mut aliasing);
        let (_, in_band_db) = peak(&in_band, 2048);
        let (_, aliasing_db) = peak(&aliasing, 2048);
        assert!(in_band_db > -1.0, "in-band droop {in_band_db} dB");
        assert!(
            in_band_db - aliasing_db >= 40.0,
            "alias rejection only {} dB",
            in_band_db - aliasing_db
        );
    }

    #[test]
    fn ddc_keeps_phase_across_calls() {
        let input_rate = 2e6;
        let input = tone(8192, 250e3, input_rate);
        let sub_band = SubBand {
            offset_hz: 250e3,
            decimation: 4,
        };
        let mut whole = Vec::new();
        Ddc::new(sub_band, input_rate).process(&input, &mut whole);
        let mut split = Vec::new();
        let mut ddc = Ddc::new(sub_band, input_rate);
        ddc.process(&input[..3000], &mut split);
        ddc.process(&input[3000..], &mut split);
        assert_eq!(whole.len(), split.len());
        for (left, right) in whole.iter().zip(&split) {
            assert!((left - right).norm() < 1e-4);
        }
    }
}
