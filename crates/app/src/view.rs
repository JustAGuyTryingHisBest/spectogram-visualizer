use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use spectra_dsp::{ChainConfig, RowFrame};

use crate::spectrum_plot::PlotMapping;

pub type ViewId = u64;
pub type SourceId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ViewMode {
    Spectrum,
    Waterfall,
    Both,
}

/// Everything a view remembers between frames; the chain and ring live beside it in the app.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct View {
    pub id: ViewId,
    pub source_id: SourceId,
    pub config: ChainConfig,
    pub ref_level_db: f32,
    pub range_db: f32,
    pub show_live: bool,
    pub show_average: bool,
    pub show_max_hold: bool,
    pub mode: ViewMode,
    #[serde(skip)]
    pub last_mapping: Option<PlotMapping>,
}

impl View {
    pub fn new(id: ViewId, source_id: SourceId) -> Self {
        Self {
            id,
            source_id,
            config: ChainConfig::default(),
            ref_level_db: 0.0,
            range_db: 120.0,
            show_live: true,
            show_average: true,
            show_max_hold: true,
            mode: ViewMode::Both,
            last_mapping: None,
        }
    }
}

/// Fixed-capacity history of rows; kept from day one so history scrubbing can land later.
pub struct RowRing {
    rows: VecDeque<RowFrame>,
    capacity: usize,
}

impl RowRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            rows: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&mut self, frame: RowFrame) {
        if self.rows.len() == self.capacity {
            self.rows.pop_front();
        }
        self.rows.push_back(frame);
    }

    pub fn latest(&self) -> Option<&RowFrame> {
        self.rows.back()
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "multi-view geometry change uses this in Task 10")
    )]
    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectra_core::{PowerUnits, SpectrumRow};
    use std::sync::Arc;
    use std::time::Instant;

    fn frame(tag: f32) -> RowFrame {
        RowFrame {
            row: SpectrumRow {
                center_hz: 0.0,
                span_hz: 1.0,
                units: PowerUnits::Dbfs,
                power_db: Arc::from(vec![tag]),
                received_at: Instant::now(),
            },
            average_db: Arc::from(vec![tag]),
            max_hold_db: None,
            source_dropped_blocks: 0,
        }
    }

    #[test]
    fn ring_keeps_only_the_newest_rows() {
        let mut ring = RowRing::new(2);
        ring.push(frame(1.0));
        ring.push(frame(2.0));
        ring.push(frame(3.0));
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.latest().unwrap().row.power_db[0], 3.0);
    }
}
