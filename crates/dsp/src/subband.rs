use num_complex::Complex32;

use crate::chain::SubBand;

/// Digital down-converter: mixes a sub-band to baseband and decimates. Implemented in Task 6.
pub struct Ddc {
    sub_band: SubBand,
    input_rate_hz: f64,
}

impl Ddc {
    pub fn new(sub_band: SubBand, input_rate_hz: f64) -> Self {
        Self {
            sub_band,
            input_rate_hz,
        }
    }

    pub fn output_rate_hz(&self) -> f64 {
        self.input_rate_hz / f64::from(self.sub_band.decimation)
    }

    pub fn process(&mut self, _input: &[Complex32], _out: &mut Vec<Complex32>) {
        unimplemented!("sub-band DDC lands in Task 6")
    }
}
