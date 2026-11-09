use serde::{Deserialize, Serialize};
use spectra_core::SpectrumRow;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub index: u8,
    pub hz: f64,
}

/// Up to nine numbered markers on one view.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Markers {
    markers: Vec<Marker>,
}

pub struct MarkerReadout {
    pub index: u8,
    pub hz: f64,
    pub db: f32,
    /// (Hz, dB) difference from the lowest-numbered marker, for every marker but that one.
    pub delta_from_first: Option<(f64, f32)>,
}

impl Markers {
    /// Places a marker at `hz` under the lowest free index 1..=9; `None` when all are taken.
    pub fn add_at(&mut self, hz: f64) -> Option<u8> {
        let index = (1..=9u8).find(|candidate| self.get(*candidate).is_none())?;
        self.markers.push(Marker { index, hz });
        self.markers.sort_by_key(|marker| marker.index);
        Some(index)
    }

    pub fn remove(&mut self, index: u8) {
        self.markers.retain(|marker| marker.index != index);
    }

    pub fn get(&self, index: u8) -> Option<&Marker> {
        self.markers.iter().find(|marker| marker.index == index)
    }

    pub fn set_hz(&mut self, index: u8, hz: f64) {
        if let Some(marker) = self.markers.iter_mut().find(|marker| marker.index == index) {
            marker.hz = hz;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Marker> {
        self.markers.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }

    /// Moves marker `index` to the strongest bin within ±`search_bins` of its current bin.
    pub fn snap_to_peak(&mut self, index: u8, row: &SpectrumRow, search_bins: usize) {
        let Some(marker) = self.get(index) else {
            return;
        };
        let Some(center_bin) = bin_for_hz(row, marker.hz) else {
            return;
        };
        let start = center_bin.saturating_sub(search_bins);
        let end = (center_bin + search_bins + 1).min(row.power_db.len());
        let peak_bin = (start..end)
            .max_by(|left, right| row.power_db[*left].total_cmp(&row.power_db[*right]))
            .unwrap_or(center_bin);
        self.set_hz(index, row.bin_hz(peak_bin));
    }
}

/// Bin index whose center is nearest `hz`, or `None` when `hz` is outside the row.
pub fn bin_for_hz(row: &SpectrumRow, hz: f64) -> Option<usize> {
    let bin_count = row.power_db.len();
    let offset = (hz - row.center_hz) / row.bin_width_hz() + bin_count as f64 / 2.0;
    let bin = offset.round();
    (bin >= 0.0 && bin < bin_count as f64).then_some(bin as usize)
}

pub fn marker_readout(markers: &Markers, row: &SpectrumRow) -> Vec<MarkerReadout> {
    let mut readouts: Vec<MarkerReadout> = Vec::new();
    for marker in markers.iter() {
        let Some(bin) = bin_for_hz(row, marker.hz) else {
            continue;
        };
        let db = row.power_db[bin];
        let delta_from_first = readouts
            .first()
            .map(|first| (marker.hz - first.hz, db - first.db));
        readouts.push(MarkerReadout {
            index: marker.index,
            hz: marker.hz,
            db,
            delta_from_first,
        });
    }
    readouts
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectra_core::{PowerUnits, SpectrumRow};
    use std::sync::Arc;
    use std::time::Instant;

    fn row(power_db: Vec<f32>) -> SpectrumRow {
        SpectrumRow {
            center_hz: 100e6,
            span_hz: 1e6,
            units: PowerUnits::Dbfs,
            power_db: Arc::from(power_db),
            received_at: Instant::now(),
        }
    }

    #[test]
    fn readout_reports_power_and_delta() {
        let mut power = vec![-90.0f32; 1000];
        power[100] = -40.0;
        power[300] = -55.0;
        let row = row(power);
        let mut markers = Markers::default();
        markers.add_at(row.bin_hz(100));
        markers.add_at(row.bin_hz(300));
        let readouts = marker_readout(&markers, &row);
        assert_eq!(readouts.len(), 2);
        assert_eq!(readouts[0].db, -40.0);
        assert_eq!(readouts[1].db, -55.0);
        assert_eq!(readouts[0].delta_from_first, None);
        let (delta_hz, delta_db) = readouts[1].delta_from_first.unwrap();
        assert!((delta_hz - 200.0 * row.bin_width_hz()).abs() < 1e-3);
        assert_eq!(delta_db, -15.0);
    }

    #[test]
    fn add_marker_uses_next_free_index_and_caps_at_nine() {
        let mut markers = Markers::default();
        for expected in 1..=9 {
            assert_eq!(markers.add_at(expected as f64), Some(expected));
        }
        assert_eq!(markers.add_at(10.0), None);
        markers.remove(5);
        assert_eq!(markers.add_at(11.0), Some(5));
    }

    #[test]
    fn snap_moves_to_local_peak() {
        let mut power = vec![-90.0f32; 1000];
        power[512] = -30.0;
        let row = row(power);
        let mut markers = Markers::default();
        let index = markers.add_at(row.bin_hz(500)).unwrap();
        markers.snap_to_peak(index, &row, 20);
        assert_eq!(markers.get(index).unwrap().hz, row.bin_hz(512));
    }
}
