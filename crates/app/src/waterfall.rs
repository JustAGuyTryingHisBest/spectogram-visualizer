use std::time::Instant;

use egui::{Align2, Color32, ColorImage, Rect, TextureHandle, TextureId, TextureOptions, pos2};

use crate::colormap::Colormap;

const LABEL_COLOR: Color32 = Color32::from_rgb(0x8a, 0x94, 0xa8);

/// GPU-backed waterfall: a ring of texture rows, newest drawn at the top.
pub struct Waterfall {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
    /// Arrival time of each texture row, for the time axis.
    row_times: Vec<Option<Instant>>,
    write_row: usize,
    rows_pushed: u64,
    texture: Option<TextureHandle>,
    /// Rows changed since the last upload; once it would exceed the height, upload everything.
    dirty_rows: Vec<usize>,
    full_upload_pending: bool,
}

impl Waterfall {
    pub fn new(width_bins: usize, height_rows: usize) -> Self {
        let width = width_bins.max(1);
        let height = height_rows.max(1);
        Self {
            width,
            height,
            pixels: vec![Color32::BLACK; width * height],
            row_times: vec![None; height],
            write_row: 0,
            rows_pushed: 0,
            texture: None,
            dirty_rows: Vec::new(),
            full_upload_pending: false,
        }
    }

    #[cfg(test)]
    pub fn rows_pushed(&self) -> u64 {
        self.rows_pushed
    }

    /// Texture row holding the newest data.
    #[cfg(test)]
    pub fn newest_index(&self) -> usize {
        (self.write_row + self.height - 1) % self.height
    }

    #[cfg(test)]
    pub fn row_pixels(&self, row_index: usize) -> &[Color32] {
        &self.pixels[row_index * self.width..(row_index + 1) * self.width]
    }

    /// Rows waiting to be uploaded; bounded by the texture height.
    #[cfg(test)]
    pub fn pending_upload_rows(&self) -> usize {
        if self.full_upload_pending {
            self.height
        } else {
            self.dirty_rows.len()
        }
    }

    #[cfg(test)]
    pub fn push_row(
        &mut self,
        power_db: &[f32],
        ref_level_db: f32,
        range_db: f32,
        colormap: &Colormap,
    ) {
        self.push_row_at(power_db, Instant::now(), ref_level_db, range_db, colormap);
    }

    /// Resamples `power_db` to the texture width by max over covered bins and appends it.
    pub fn push_row_at(
        &mut self,
        power_db: &[f32],
        received_at: Instant,
        ref_level_db: f32,
        range_db: f32,
        colormap: &Colormap,
    ) {
        let bin_count = power_db.len().max(1);
        let row_start = self.write_row * self.width;
        for column in 0..self.width {
            let start = column * bin_count / self.width;
            let end = ((column + 1) * bin_count / self.width)
                .max(start + 1)
                .min(bin_count);
            let max_db = power_db[start..end]
                .iter()
                .fold(f32::MIN, |max, db| max.max(*db));
            let normalized = (max_db - (ref_level_db - range_db)) / range_db;
            self.pixels[row_start + column] = colormap.lookup(normalized);
        }
        self.row_times[self.write_row] = Some(received_at);
        if self.full_upload_pending || self.dirty_rows.len() + 1 >= self.height {
            self.full_upload_pending = true;
            self.dirty_rows.clear();
        } else {
            self.dirty_rows.push(self.write_row);
        }
        self.write_row = (self.write_row + 1) % self.height;
        self.rows_pushed += 1;
    }

    fn image(&self, rows: std::ops::Range<usize>) -> ColorImage {
        ColorImage {
            size: [self.width, rows.len()],
            source_size: egui::vec2(self.width as f32, rows.len() as f32),
            pixels: self.pixels[rows.start * self.width..rows.end * self.width].to_vec(),
        }
    }

    pub fn ensure_texture(&mut self, ctx: &egui::Context) -> TextureId {
        let options = TextureOptions::NEAREST;
        if self.texture.is_none() {
            self.texture = Some(ctx.load_texture("waterfall", self.image(0..self.height), options));
            self.dirty_rows.clear();
            self.full_upload_pending = false;
        }
        let texture = self.texture.as_mut().expect("texture created above");
        if self.full_upload_pending {
            let whole = ColorImage {
                size: [self.width, self.height],
                source_size: egui::vec2(self.width as f32, self.height as f32),
                pixels: self.pixels.clone(),
            };
            texture.set(whole, options);
            self.full_upload_pending = false;
        } else {
            for row in std::mem::take(&mut self.dirty_rows) {
                let image = ColorImage {
                    size: [self.width, 1],
                    source_size: egui::vec2(self.width as f32, 1.0),
                    pixels: self.pixels[row * self.width..(row + 1) * self.width].to_vec(),
                };
                texture.set_partial([0, row], image, options);
            }
        }
        texture.id()
    }

    /// Draws the ring into `rect` with the newest row at the top, as two quads around the wrap
    /// point, plus a time axis on the right and the colormap bar bottom-left.
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        ref_level_db: f32,
        range_db: f32,
        colormap: &Colormap,
    ) {
        let texture_id = self.ensure_texture(ui.ctx());
        let painter = ui.painter_at(rect);
        let split_v = self.write_row as f32 / self.height as f32;
        // Rows write_row..height are the oldest, 0..write_row the newest; newest goes on top.
        let newest_height = rect.height() * split_v;
        if self.write_row > 0 {
            let newest_rect =
                Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + newest_height));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, split_v));
            painter.image(texture_id, newest_rect, flip_vertical(uv), Color32::WHITE);
        }
        if self.write_row < self.height {
            let oldest_rect =
                Rect::from_min_max(pos2(rect.min.x, rect.min.y + newest_height), rect.max);
            let uv = Rect::from_min_max(pos2(0.0, split_v), pos2(1.0, 1.0));
            painter.image(texture_id, oldest_rect, flip_vertical(uv), Color32::WHITE);
        }
        self.draw_time_axis(&painter, rect);
        draw_colormap_bar(&painter, rect, ref_level_db, range_db, colormap);
    }

    /// Labels a few screen rows with their age relative to the newest row.
    fn draw_time_axis(&self, painter: &egui::Painter, rect: Rect) {
        let newest_index = (self.write_row + self.height - 1) % self.height;
        let Some(newest_time) = self.row_times[newest_index] else {
            return;
        };
        let font = egui::FontId::monospace(10.0);
        let rows_on_screen = (self.rows_pushed as usize).min(self.height);
        for fraction in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let rows_back = ((rows_on_screen.saturating_sub(1)) as f32 * fraction) as usize;
            let row_index = (newest_index + self.height - rows_back) % self.height;
            let Some(time) = self.row_times[row_index] else {
                continue;
            };
            let age = newest_time.saturating_duration_since(time).as_secs_f32();
            let label = if rows_back == 0 {
                "now".to_string()
            } else {
                format!("-{age:.1} s")
            };
            let y = rect.top() + rect.height() * (rows_back as f32 / self.height as f32);
            painter.text(
                pos2(
                    rect.right() - 4.0,
                    y.clamp(rect.top(), rect.bottom() - 12.0),
                ),
                Align2::RIGHT_TOP,
                label,
                font.clone(),
                LABEL_COLOR,
            );
        }
    }
}

fn draw_colormap_bar(
    painter: &egui::Painter,
    rect: Rect,
    ref_level_db: f32,
    range_db: f32,
    colormap: &Colormap,
) {
    let bar = Rect::from_min_size(
        pos2(rect.left() + 6.0, rect.bottom() - 14.0),
        egui::vec2(120.0, 8.0),
    );
    let steps = 32;
    for step in 0..steps {
        let fraction = step as f32 / steps as f32;
        let segment = Rect::from_min_max(
            pos2(bar.left() + bar.width() * fraction, bar.top()),
            pos2(
                bar.left() + bar.width() * (step + 1) as f32 / steps as f32,
                bar.bottom(),
            ),
        );
        painter.rect_filled(segment, 0.0, colormap.lookup(fraction));
    }
    let font = egui::FontId::monospace(9.0);
    painter.text(
        pos2(bar.left(), bar.top() - 2.0),
        Align2::LEFT_BOTTOM,
        format!("{:.0}", ref_level_db - range_db),
        font.clone(),
        LABEL_COLOR,
    );
    painter.text(
        pos2(bar.right(), bar.top() - 2.0),
        Align2::RIGHT_BOTTOM,
        format!("{ref_level_db:.0} dB"),
        font,
        LABEL_COLOR,
    );
}

/// Rows are written top-down in texture order but the newest row must appear highest on screen.
fn flip_vertical(uv: Rect) -> Rect {
    Rect::from_min_max(pos2(uv.min.x, uv.max.y), pos2(uv.max.x, uv.min.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waterfall_ring_wraps() {
        let colormap = Colormap::inferno();
        let mut waterfall = Waterfall::new(8, 4);
        for row_index in 0..6 {
            let row = vec![-50.0 - row_index as f32; 16];
            waterfall.push_row(&row, 0.0, 120.0, &colormap);
        }
        assert_eq!(waterfall.rows_pushed(), 6);
        assert_eq!(waterfall.newest_index(), 1);
    }

    #[test]
    fn dirty_rows_stay_bounded_when_not_drawn() {
        let colormap = Colormap::inferno();
        let mut waterfall = Waterfall::new(4, 8);
        for _ in 0..10_000 {
            waterfall.push_row(&[-50.0; 4], 0.0, 120.0, &colormap);
        }
        assert!(waterfall.pending_upload_rows() <= 8);
    }

    #[test]
    fn push_row_resamples_by_max_over_covered_bins() {
        let colormap = Colormap::inferno();
        let mut waterfall = Waterfall::new(2, 1);
        waterfall.push_row(&[-120.0, -120.0, -120.0, 0.0], 0.0, 120.0, &colormap);
        let pixels = waterfall.row_pixels(0);
        assert_eq!(pixels[0], colormap.lookup(0.0));
        assert_eq!(pixels[1], colormap.lookup(1.0));
    }
}
