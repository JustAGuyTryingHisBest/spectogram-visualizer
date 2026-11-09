use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use spectra_dsp::{ChainConfig, ChainWorker, RowFrame};

use crate::colormap::Colormap;
use crate::markers::Markers;
use crate::spectrum_plot::PlotMapping;
use crate::waterfall::Waterfall;

pub const ROW_RING_CAPACITY: usize = 4096;
pub const WATERFALL_WIDTH: usize = 2048;
pub const WATERFALL_HEIGHT: usize = 1024;

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
    pub markers: Markers,
    #[serde(skip)]
    pub last_mapping: Option<PlotMapping>,
    #[serde(skip)]
    pub dragging_marker: Option<u8>,
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
            markers: Markers::default(),
            last_mapping: None,
            dragging_marker: None,
        }
    }
}

/// A view plus the live state behind it: its chain worker, row history and waterfall texture.
pub struct ViewRuntime {
    pub view: View,
    pub worker: Option<ChainWorker>,
    pub ring: RowRing,
    pub waterfall: Waterfall,
    last_geometry: Option<(f64, usize)>,
}

impl ViewRuntime {
    pub fn new(view: View) -> Self {
        Self {
            view,
            worker: None,
            ring: RowRing::new(ROW_RING_CAPACITY),
            waterfall: Waterfall::new(WATERFALL_WIDTH, WATERFALL_HEIGHT),
            last_geometry: None,
        }
    }

    /// Stores a row; a change in span or bin count discards history so axes never go stale.
    pub fn ingest(&mut self, frame: RowFrame, colormap: &Colormap) {
        let geometry = (frame.row.span_hz, frame.row.power_db.len());
        if self
            .last_geometry
            .is_some_and(|previous| previous != geometry)
        {
            self.ring.clear();
            self.waterfall = Waterfall::new(WATERFALL_WIDTH, WATERFALL_HEIGHT);
        }
        self.last_geometry = Some(geometry);
        self.waterfall.push_row(
            &frame.row.power_db,
            self.view.ref_level_db,
            self.view.range_db,
            colormap,
        );
        self.ring.push(frame);
    }

    pub fn drain_worker(&mut self, colormap: &Colormap) {
        let mut frames = Vec::new();
        if let Some(worker) = &self.worker {
            while let Some(frame) = worker.rows().try_recv() {
                frames.push(frame);
            }
        }
        for frame in frames {
            self.ingest(frame, colormap);
        }
    }

    pub fn apply_config(&mut self) {
        if let Some(worker) = &self.worker {
            worker.set_config(self.view.config.clone());
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

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn clear(&mut self) {
        self.rows.clear();
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

    fn frame_with_span(span_hz: f64, bins: usize) -> RowFrame {
        RowFrame {
            row: SpectrumRow {
                center_hz: 0.0,
                span_hz,
                units: PowerUnits::Dbfs,
                power_db: Arc::from(vec![-80.0; bins]),
                received_at: Instant::now(),
            },
            average_db: Arc::from(vec![-80.0; bins]),
            max_hold_db: None,
            source_dropped_blocks: 0,
        }
    }

    #[test]
    fn geometry_change_clears_ring() {
        let colormap = Colormap::inferno();
        let mut runtime = ViewRuntime::new(View::new(1, 1));
        runtime.ingest(frame_with_span(2e6, 1024), &colormap);
        runtime.ingest(frame_with_span(2e6, 1024), &colormap);
        assert_eq!(runtime.ring.len(), 2);
        runtime.ingest(frame_with_span(1e6, 1024), &colormap);
        assert_eq!(runtime.ring.len(), 1);
        assert_eq!(runtime.waterfall.rows_pushed(), 1);
        runtime.ingest(frame_with_span(1e6, 512), &colormap);
        assert_eq!(runtime.ring.len(), 1);
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
