use egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ColormapKind {
    Inferno,
    Viridis,
    Grayscale,
}

impl ColormapKind {
    pub const ALL: [ColormapKind; 3] = [
        ColormapKind::Inferno,
        ColormapKind::Viridis,
        ColormapKind::Grayscale,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ColormapKind::Inferno => "inferno",
            ColormapKind::Viridis => "viridis",
            ColormapKind::Grayscale => "gray",
        }
    }
}

/// 256-entry lookup table from a normalized value in 0..=1 to a color.
pub struct Colormap {
    table: Vec<Color32>,
}

/// Matplotlib's inferno and viridis as Matt Zucker's degree-6 polynomial fits.
const INFERNO: [[f32; 3]; 7] = [
    [0.000_218_940_37, 0.001_651_004_6, -0.019_480_898],
    [0.106_513_42, 0.563_956_44, 3.932_712_4],
    [11.602_493, -3.972_854, -15.942_394],
    [-41.703_996, 17.436_4, 44.354_145],
    [77.162_94, -33.402_36, -81.807_31],
    [-71.319_43, 32.626_064, 73.209_52],
    [25.131_126, -12.242_669, -23.070_325],
];
const VIRIDIS: [[f32; 3]; 7] = [
    [0.277_727_1, 0.005_407_344, 0.334_099_8],
    [0.105_093_26, 1.404_613_5, 1.384_590_7],
    [-0.330_861_5, 0.214_847_5, 0.095_095_16],
    [-4.634_230_6, -5.799_1, -19.332_445],
    [6.228_269_5, 14.179_933, 56.690_55],
    [4.776_384_5, -13.745_145, -65.353_035],
    [-5.435_455, 4.645_852, 26.312_435],
];

impl Colormap {
    pub fn new(kind: ColormapKind) -> Self {
        let table = (0..256)
            .map(|index| {
                let t = index as f32 / 255.0;
                let rgb = match kind {
                    ColormapKind::Inferno => polynomial(&INFERNO, t),
                    ColormapKind::Viridis => polynomial(&VIRIDIS, t),
                    ColormapKind::Grayscale => [t, t, t],
                };
                let to_byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
                Color32::from_rgb(to_byte(rgb[0]), to_byte(rgb[1]), to_byte(rgb[2]))
            })
            .collect();
        Self { table }
    }

    #[cfg(test)]
    pub fn inferno() -> Self {
        Self::new(ColormapKind::Inferno)
    }

    pub fn lookup(&self, normalized: f32) -> Color32 {
        let index = (normalized.clamp(0.0, 1.0) * 255.0).round() as usize;
        self.table[index.min(255)]
    }
}

fn polynomial(coefficients: &[[f32; 3]; 7], t: f32) -> [f32; 3] {
    let mut rgb = [0.0f32; 3];
    let mut power = 1.0;
    for coefficient in coefficients {
        for (channel, value) in rgb.iter_mut().zip(coefficient) {
            *channel += value * power;
        }
        power *= t;
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: egui::Color32) -> f32 {
        0.2126 * color.r() as f32 + 0.7152 * color.g() as f32 + 0.0722 * color.b() as f32
    }

    #[test]
    fn colormap_clamps_and_is_monotonic_in_luminance() {
        for kind in ColormapKind::ALL {
            let colormap = Colormap::new(kind);
            assert_eq!(colormap.lookup(-1.0), colormap.lookup(0.0));
            assert_eq!(colormap.lookup(2.0), colormap.lookup(1.0));
            let dark = luminance(colormap.lookup(0.0));
            let mid = luminance(colormap.lookup(0.5));
            let bright = luminance(colormap.lookup(1.0));
            assert!(
                dark < mid && mid < bright,
                "{kind:?}: {dark} {mid} {bright}"
            );
        }
    }
}
