use egui::{Color32, ColorImage, Rect, TextureHandle, TextureId, TextureOptions, pos2};

use crate::colormap::Colormap;

/// GPU-backed waterfall: a ring of texture rows, newest drawn at the top.
pub struct Waterfall {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
    write_row: usize,
    rows_pushed: u64,
    texture: Option<TextureHandle>,
    dirty_rows: Vec<usize>,
}

impl Waterfall {
    pub fn new(width_bins: usize, height_rows: usize) -> Self {
        let width = width_bins.max(1);
        let height = height_rows.max(1);
        Self {
            width,
            height,
            pixels: vec![Color32::BLACK; width * height],
            write_row: 0,
            rows_pushed: 0,
            texture: None,
            dirty_rows: Vec::new(),
        }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "geometry-change handling uses this in Task 10")
    )]
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

    /// Resamples `power_db` to the texture width by max over covered bins and appends it.
    pub fn push_row(
        &mut self,
        power_db: &[f32],
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
        self.dirty_rows.push(self.write_row);
        self.write_row = (self.write_row + 1) % self.height;
        self.rows_pushed += 1;
    }

    pub fn ensure_texture(&mut self, ctx: &egui::Context) -> TextureId {
        let options = TextureOptions::NEAREST;
        let texture = self.texture.get_or_insert_with(|| {
            self.dirty_rows.clear();
            ctx.load_texture(
                "waterfall",
                ColorImage {
                    size: [self.width, self.height],
                    source_size: egui::vec2(self.width as f32, self.height as f32),
                    pixels: self.pixels.clone(),
                },
                options,
            )
        });
        if self.dirty_rows.len() >= self.height {
            texture.set(
                ColorImage {
                    size: [self.width, self.height],
                    source_size: egui::vec2(self.width as f32, self.height as f32),
                    pixels: self.pixels.clone(),
                },
                options,
            );
        } else {
            for row in self.dirty_rows.drain(..) {
                texture.set_partial(
                    [0, row],
                    ColorImage {
                        size: [self.width, 1],
                        source_size: egui::vec2(self.width as f32, 1.0),
                        pixels: self.pixels[row * self.width..(row + 1) * self.width].to_vec(),
                    },
                    options,
                );
            }
        }
        self.dirty_rows.clear();
        texture.id()
    }

    /// Draws the ring into `rect` with the newest row at the top, as two quads around the wrap point.
    pub fn draw(&mut self, ui: &mut egui::Ui, rect: Rect) {
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
    }
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
    fn push_row_resamples_by_max_over_covered_bins() {
        let colormap = Colormap::inferno();
        let mut waterfall = Waterfall::new(2, 1);
        waterfall.push_row(&[-120.0, -120.0, -120.0, 0.0], 0.0, 120.0, &colormap);
        let pixels = waterfall.row_pixels(0);
        assert_eq!(pixels[0], colormap.lookup(0.0));
        assert_eq!(pixels[1], colormap.lookup(1.0));
    }
}
