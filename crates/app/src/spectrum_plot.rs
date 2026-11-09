use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2};
use spectra_dsp::RowFrame;

use crate::markers::marker_readout;
use crate::source_panel::format_hz;
use crate::view::View;

pub const LIVE_COLOR: Color32 = Color32::from_rgb(0x8f, 0xb6, 0xff);
pub const AVERAGE_COLOR: Color32 = Color32::from_rgb(0x6f, 0xd4, 0x9a);
pub const MAX_HOLD_COLOR: Color32 = Color32::from_rgb(0xff, 0x8c, 0x42);
pub const MARKER_COLOR: Color32 = Color32::from_rgb(0xff, 0xd3, 0x6b);
const MARKER_GRAB_PX: f32 = 6.0;
const PEAK_SNAP_BINS: usize = 20;
const GRID_COLOR: Color32 = Color32::from_rgb(0x1e, 0x25, 0x34);
const LABEL_COLOR: Color32 = Color32::from_rgb(0x8a, 0x94, 0xa8);
const BACKGROUND: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x14);

/// Maps frequency and power to pixels inside a plot rectangle.
#[derive(Clone, Copy, Debug)]
pub struct PlotMapping {
    rect: Rect,
    low_hz: f64,
    high_hz: f64,
    low_db: f32,
    high_db: f32,
}

impl PlotMapping {
    pub fn new(rect: Rect, low_hz: f64, high_hz: f64, low_db: f32, high_db: f32) -> Self {
        Self {
            rect,
            low_hz,
            high_hz,
            low_db,
            high_db,
        }
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn x_for_hz(&self, hz: f64) -> f32 {
        let fraction = (hz - self.low_hz) / (self.high_hz - self.low_hz);
        self.rect.left() + fraction as f32 * self.rect.width()
    }

    pub fn hz_for_x(&self, x: f32) -> f64 {
        let fraction = f64::from((x - self.rect.left()) / self.rect.width());
        self.low_hz + fraction * (self.high_hz - self.low_hz)
    }

    pub fn y_for_db(&self, db: f32) -> f32 {
        let fraction = (self.high_db - db) / (self.high_db - self.low_db);
        self.rect.top() + fraction * self.rect.height()
    }
}

/// Draws the spectrum traces, grid and axis labels for `view` into the available space.
pub fn draw_spectrum(
    ui: &mut egui::Ui,
    view: &mut View,
    frame: Option<&RowFrame>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, BACKGROUND);

    let Some(frame) = frame else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "waiting for data",
            egui::FontId::monospace(12.0),
            LABEL_COLOR,
        );
        return response;
    };

    let row = &frame.row;
    let low_hz = row.center_hz - row.span_hz / 2.0;
    let high_hz = row.center_hz + row.span_hz / 2.0;
    let high_db = view.ref_level_db;
    let low_db = view.ref_level_db - view.range_db;
    let mapping = PlotMapping::new(rect, low_hz, high_hz, low_db, high_db);
    view.last_mapping = Some(mapping);

    draw_grid(&painter, &mapping, row.units);
    if let (true, Some(max_hold)) = (view.show_max_hold, &frame.max_hold_db) {
        draw_trace(&painter, &mapping, max_hold, MAX_HOLD_COLOR);
    }
    if view.show_average {
        draw_trace(&painter, &mapping, &frame.average_db, AVERAGE_COLOR);
    }
    if view.show_live {
        draw_trace(&painter, &mapping, &row.power_db, LIVE_COLOR);
    }

    handle_marker_input(ui, view, &mapping, row, &response);
    draw_markers(&painter, &mapping, view, row);
    response
}

/// Left click adds a marker, dragging a marker line moves it, right click removes it,
/// and `P` snaps the marker under the pointer to the nearest peak.
fn handle_marker_input(
    ui: &egui::Ui,
    view: &mut View,
    mapping: &PlotMapping,
    row: &spectra_core::SpectrumRow,
    response: &egui::Response,
) {
    let Some(pointer) = response
        .interact_pointer_pos()
        .or_else(|| response.hover_pos())
    else {
        if !response.dragged() {
            view.dragging_marker = None;
        }
        return;
    };
    let nearest = view
        .markers
        .iter()
        .map(|marker| {
            (
                marker.index,
                (mapping.x_for_hz(marker.hz) - pointer.x).abs(),
            )
        })
        .filter(|(_, distance)| *distance <= MARKER_GRAB_PX)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index);

    if response.drag_started() {
        view.dragging_marker = nearest;
    }
    if response.dragged()
        && let Some(index) = view.dragging_marker
    {
        view.markers.set_hz(index, mapping.hz_for_x(pointer.x));
    }
    if response.drag_stopped() {
        view.dragging_marker = None;
    }
    if response.clicked() {
        if nearest.is_none() {
            view.markers.add_at(mapping.hz_for_x(pointer.x));
        }
    } else if response.secondary_clicked()
        && let Some(index) = nearest
    {
        view.markers.remove(index);
    }
    if ui.input(|input| input.key_pressed(egui::Key::P))
        && let Some(index) = nearest
    {
        view.markers.snap_to_peak(index, row, PEAK_SNAP_BINS);
    }
}

fn draw_markers(
    painter: &egui::Painter,
    mapping: &PlotMapping,
    view: &View,
    row: &spectra_core::SpectrumRow,
) {
    if view.markers.is_empty() {
        return;
    }
    let rect = mapping.rect();
    let font = egui::FontId::monospace(11.0);
    let readouts = marker_readout(&view.markers, row);
    for readout in &readouts {
        let x = mapping.x_for_hz(readout.hz);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, MARKER_COLOR),
        );
        let y = mapping.y_for_db(readout.db);
        painter.circle_filled(pos2(x, y), 3.0, MARKER_COLOR);
        painter.text(
            pos2(x + 4.0, rect.top() + 14.0),
            egui::Align2::LEFT_TOP,
            format!("M{}", readout.index),
            font.clone(),
            MARKER_COLOR,
        );
    }
    let mut lines = Vec::new();
    for readout in &readouts {
        let mut line = format!(
            "M{} {} {:.1} dB",
            readout.index,
            format_hz(readout.hz),
            readout.db
        );
        if let Some((delta_hz, delta_db)) = readout.delta_from_first {
            line.push_str(&format!("  Δ {} {:+.1} dB", format_hz(delta_hz), delta_db));
        }
        lines.push(line);
    }
    painter.text(
        pos2(rect.right() - 6.0, rect.top() + 14.0),
        egui::Align2::RIGHT_TOP,
        lines.join("\n"),
        font,
        MARKER_COLOR,
    );
}

fn draw_grid(painter: &egui::Painter, mapping: &PlotMapping, units: spectra_core::PowerUnits) {
    let rect = mapping.rect();
    let stroke = Stroke::new(1.0, GRID_COLOR);
    let font = egui::FontId::monospace(10.0);
    let mut db = (mapping.high_db / 10.0).floor() * 10.0;
    while db > mapping.low_db {
        let y = mapping.y_for_db(db);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], stroke);
        painter.text(
            pos2(rect.left() + 4.0, y - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:.0}"),
            font.clone(),
            LABEL_COLOR,
        );
        db -= 10.0;
    }
    for division in 1..10 {
        let x = rect.left() + rect.width() * division as f32 / 10.0;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], stroke);
    }
    let unit_label = match units {
        spectra_core::PowerUnits::Dbfs => "dBFS",
        spectra_core::PowerUnits::Dbm => "dBm",
    };
    painter.text(
        pos2(rect.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        unit_label,
        font.clone(),
        LABEL_COLOR,
    );
    let labels = [
        (mapping.low_hz, egui::Align2::LEFT_BOTTOM, rect.left() + 4.0),
        (
            (mapping.low_hz + mapping.high_hz) / 2.0,
            egui::Align2::CENTER_BOTTOM,
            rect.center().x,
        ),
        (
            mapping.high_hz,
            egui::Align2::RIGHT_BOTTOM,
            rect.right() - 4.0,
        ),
    ];
    for (hz, align, x) in labels {
        painter.text(
            pos2(x, rect.bottom() - 2.0),
            align,
            format!("{:.3} MHz", hz / 1e6),
            font.clone(),
            LABEL_COLOR,
        );
    }
}

/// One min/max pair per pixel column when there are more bins than pixels, else one point per bin.
fn draw_trace(painter: &egui::Painter, mapping: &PlotMapping, power_db: &[f32], color: Color32) {
    let rect = mapping.rect();
    let width_px = rect.width().max(1.0) as usize;
    let bin_count = power_db.len();
    let mut points: Vec<Pos2> = Vec::with_capacity(width_px.min(bin_count) * 2);
    if bin_count <= width_px {
        for (index, db) in power_db.iter().enumerate() {
            let x = rect.left() + (index as f32 + 0.5) / bin_count as f32 * rect.width();
            points.push(pos2(x, mapping.y_for_db(*db)));
        }
    } else {
        for column in 0..width_px {
            let start = column * bin_count / width_px;
            let end = ((column + 1) * bin_count / width_px).max(start + 1);
            let slice = &power_db[start..end];
            let (min, max) = slice.iter().fold((f32::MAX, f32::MIN), |(lo, hi), db| {
                (lo.min(*db), hi.max(*db))
            });
            let x = rect.left() + (column as f32 + 0.5);
            points.push(pos2(x, mapping.y_for_db(max)));
            if (max - min).abs() > 0.01 {
                points.push(pos2(x, mapping.y_for_db(min)));
            }
        }
    }
    painter.add(Shape::line(points, Stroke::new(1.0, color)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_round_trips() {
        let mapping = PlotMapping::new(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 200.0)),
            905e6,
            925e6,
            -110.0,
            -40.0,
        );
        assert_eq!(mapping.x_for_hz(915e6), 400.0);
        assert!((mapping.hz_for_x(mapping.x_for_hz(910.5e6)) - 910.5e6).abs() < 1.0);
        assert_eq!(mapping.y_for_db(-40.0), 0.0);
        assert_eq!(mapping.y_for_db(-110.0), 200.0);
    }
}
