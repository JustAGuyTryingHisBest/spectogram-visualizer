use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use spectra_core::{SpectrumRow, Subscriber};
use spectra_dsp::{ChainConfig, ChainWorker, RowFrame, TraceAccumulator};

use crate::colormap::{Colormap, ColormapKind};
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
    pub colormap: ColormapKind,
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
            colormap: ColormapKind::Inferno,
            markers: Markers::default(),
            last_mapping: None,
            dragging_marker: None,
        }
    }
}

/// Display rows per second the view keeps; chain rows arriving faster are max-combined.
pub const DISPLAY_ROWS_PER_SECOND: f64 = 100.0;
/// Upper bound on what one view's row history may hold, across live/average/max-hold rows.
pub const ROW_RING_BYTES_BUDGET: usize = 128 * 1024 * 1024;

/// A view plus the live state behind it: its chain worker, row history and waterfall texture.
pub struct ViewRuntime {
    pub view: View,
    /// Chain worker for IQ sources; `None` for spectrum sources, which feed `spectrum_input`.
    pub worker: Option<ChainWorker>,
    pub spectrum_input: Option<Subscriber<SpectrumRow>>,
    traces: TraceAccumulator,
    pub ring: RowRing,
    pub waterfall: Waterfall,
    pub colormap: Colormap,
    last_geometry: Option<(f64, f64, usize)>,
    /// Chain rows max-combined since the last display row was emitted.
    pending_max_db: Vec<f32>,
    last_display_row_at: Option<std::time::Instant>,
}

impl ViewRuntime {
    pub fn new(view: View) -> Self {
        Self {
            traces: TraceAccumulator::new(view.config.averaging, view.config.max_hold),
            colormap: Colormap::new(view.colormap),
            view,
            worker: None,
            spectrum_input: None,
            ring: RowRing::new(ROW_RING_CAPACITY),
            waterfall: Waterfall::new(WATERFALL_WIDTH, WATERFALL_HEIGHT),
            last_geometry: None,
            pending_max_db: Vec::new(),
            last_display_row_at: None,
        }
    }

    /// Takes one chain row. Rows are max-combined into display rows at `DISPLAY_ROWS_PER_SECOND`,
    /// and a change in center, span or bin count discards history so axes never go stale.
    pub fn ingest(&mut self, frame: RowFrame) {
        let geometry = (
            frame.row.center_hz,
            frame.row.span_hz,
            frame.row.power_db.len(),
        );
        if self
            .last_geometry
            .is_some_and(|previous| previous != geometry)
        {
            self.ring = RowRing::new(RowRing::capacity_for_bins(frame.row.power_db.len()));
            self.waterfall = Waterfall::new(WATERFALL_WIDTH, WATERFALL_HEIGHT);
            self.last_display_row_at = None;
            self.pending_max_db.clear();
        } else if self.last_geometry.is_none() {
            self.ring = RowRing::new(RowRing::capacity_for_bins(frame.row.power_db.len()));
        }
        self.last_geometry = Some(geometry);

        if self.pending_max_db.len() != frame.row.power_db.len() {
            self.pending_max_db.clear();
            self.pending_max_db.extend_from_slice(&frame.row.power_db);
        } else {
            for (held, live) in self
                .pending_max_db
                .iter_mut()
                .zip(frame.row.power_db.iter())
            {
                *held = held.max(*live);
            }
        }
        let interval = std::time::Duration::from_secs_f64(1.0 / DISPLAY_ROWS_PER_SECOND);
        let due = match self.last_display_row_at {
            None => true,
            Some(last) => frame.row.received_at.saturating_duration_since(last) >= interval,
        };
        if !due {
            return;
        }

        let display_row = SpectrumRow {
            power_db: std::sync::Arc::from(self.pending_max_db.as_slice()),
            ..frame.row.clone()
        };
        self.last_display_row_at = Some(frame.row.received_at);
        self.pending_max_db.clear();
        self.waterfall.push_row_at(
            &display_row.power_db,
            display_row.received_at,
            self.view.ref_level_db,
            self.view.range_db,
            &self.colormap,
        );
        self.ring.push(RowFrame {
            row: display_row,
            ..frame
        });
    }

    pub fn drain_worker(&mut self) {
        let mut frames = Vec::new();
        if let Some(worker) = &self.worker {
            while let Some(frame) = worker.rows().try_recv() {
                frames.push(frame);
            }
        }
        if let Some(input) = &self.spectrum_input {
            while let Some(row) = input.try_recv() {
                let (average_db, max_hold_db) = self.traces.push(&row.power_db);
                frames.push(RowFrame {
                    row,
                    average_db,
                    max_hold_db,
                    source_dropped_blocks: input.dropped(),
                });
            }
        }
        for frame in frames {
            self.ingest(frame);
        }
    }

    pub fn apply_config(&mut self) {
        if let Some(worker) = &self.worker {
            worker.set_config(self.view.config.clone());
        }
        self.traces = TraceAccumulator::new(self.view.config.averaging, self.view.config.max_hold);
    }

    pub fn apply_colormap(&mut self) {
        self.colormap = Colormap::new(self.view.colormap);
    }

    pub fn reset_max_hold(&mut self) {
        if let Some(worker) = &self.worker {
            worker.reset_max_hold();
        }
        self.traces.reset_max_hold();
    }
}

/// Fixed-capacity history of display rows; kept from day one so history scrubbing can land later.
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

    /// Rows that fit `ROW_RING_BYTES_BUDGET` for this bin count, between 16 and the default.
    pub fn capacity_for_bins(bins: usize) -> usize {
        let bytes_per_row = bins.max(1) * std::mem::size_of::<f32>() * 3;
        (ROW_RING_BYTES_BUDGET / bytes_per_row).clamp(16, ROW_RING_CAPACITY)
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

    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &RowFrame> {
        self.rows.iter()
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
        let mut runtime = ViewRuntime::new(View::new(1, 1));
        let start = Instant::now();
        let spaced = |index: u64, span_hz: f64, bins: usize| {
            let mut frame = frame_with_span(span_hz, bins);
            frame.row.received_at = start + std::time::Duration::from_millis(index * 20);
            frame
        };
        runtime.ingest(spaced(0, 2e6, 1024));
        runtime.ingest(spaced(1, 2e6, 1024));
        assert_eq!(runtime.ring.len(), 2);
        runtime.ingest(spaced(2, 1e6, 1024));
        assert_eq!(runtime.ring.len(), 1);
        assert_eq!(runtime.waterfall.rows_pushed(), 1);
        runtime.ingest(spaced(3, 1e6, 512));
        assert_eq!(runtime.ring.len(), 1);
    }

    fn frame_at(received_at: Instant, bins: usize, level_db: f32) -> RowFrame {
        let mut frame = frame_with_span(1e6, bins);
        frame.row.received_at = received_at;
        frame.row.power_db = Arc::from(vec![level_db; bins]);
        frame
    }

    #[test]
    fn display_rows_are_combined_to_target_rate() {
        let mut runtime = ViewRuntime::new(View::new(1, 1));
        let start = Instant::now();
        // 1000 chain rows spread over one second, one of them a short burst.
        for index in 0..1000 {
            let level = if index == 500 { -20.0 } else { -90.0 };
            let received_at = start + std::time::Duration::from_micros(index * 1000);
            runtime.ingest(frame_at(received_at, 64, level));
        }
        let display_rows = runtime.ring.len();
        assert!(
            (90..=110).contains(&display_rows),
            "expected about 100 display rows, got {display_rows}"
        );
        assert_eq!(runtime.waterfall.rows_pushed(), display_rows as u64);
        // Max-combining keeps the burst visible in exactly one display row.
        let bursts = runtime
            .ring
            .iter()
            .filter(|frame| frame.row.power_db[0] > -30.0)
            .count();
        assert_eq!(bursts, 1);
    }

    #[test]
    fn ring_capacity_shrinks_for_large_rows() {
        assert_eq!(RowRing::capacity_for_bins(2048), ROW_RING_CAPACITY);
        let huge = RowRing::capacity_for_bins(65536);
        assert!(huge < ROW_RING_CAPACITY);
        assert!(huge * 65536 * 4 * 3 <= ROW_RING_BYTES_BUDGET);
        assert!(huge >= 16);
    }

    #[test]
    fn spectrum_source_rows_get_traces_without_a_worker() {
        let broadcast = spectra_core::Broadcast::new();
        let mut runtime = ViewRuntime::new(View::new(1, 1));
        runtime.spectrum_input = Some(broadcast.subscribe(8));
        broadcast.publish(frame_with_span(1e6, 64).row);
        runtime.drain_worker();
        let latest = runtime.ring.latest().unwrap();
        assert_eq!(latest.average_db.len(), 64);
        assert!(latest.max_hold_db.is_some());
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
