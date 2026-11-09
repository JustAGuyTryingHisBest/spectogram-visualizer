use std::path::Path;
use std::time::Instant;

use spectra_core::SourceHandle;
use spectra_dsp::ChainWorker;
use spectra_sources::SeifySource;

use crate::colormap::Colormap;
use crate::source_panel::draw_source_panel;
use crate::sources::SourceManager;
use crate::spectrum_plot::draw_spectrum;
use crate::view::{RowRing, SourceId, View, ViewMode};
use crate::waterfall::Waterfall;

const ROW_RING_CAPACITY: usize = 4096;
const WATERFALL_WIDTH: usize = 2048;
const WATERFALL_HEIGHT: usize = 1024;

pub struct App {
    sources: SourceManager,
    worker: Option<ChainWorker>,
    view: View,
    ring: RowRing,
    waterfall: Waterfall,
    colormap: Colormap,
    frame_times: Vec<Instant>,
    error: Option<String>,
}

impl App {
    pub fn new(sigmf_path: Option<&Path>) -> Self {
        let mut app = Self {
            sources: SourceManager::default(),
            worker: None,
            view: View::new(1, 0),
            ring: RowRing::new(ROW_RING_CAPACITY),
            waterfall: Waterfall::new(WATERFALL_WIDTH, WATERFALL_HEIGHT),
            colormap: Colormap::inferno(),
            frame_times: Vec::new(),
            error: None,
        };
        if let Some(path) = sigmf_path {
            app.open_sigmf(path);
        }
        app
    }

    fn open_sigmf(&mut self, path: &Path) {
        match self.sources.open_sigmf(path) {
            Ok(id) => self.attach_view_to(id),
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    /// Points the single view at `source_id` and starts a fresh chain worker on it.
    fn attach_view_to(&mut self, source_id: SourceId) {
        let Some(SourceHandle::Iq(source)) = self.sources.get(source_id) else {
            return;
        };
        self.worker = Some(ChainWorker::spawn(
            source.subscribe(),
            self.view.config.clone(),
            256,
        ));
        self.view.source_id = source_id;
        self.ring = RowRing::new(ROW_RING_CAPACITY);
        self.waterfall = Waterfall::new(WATERFALL_WIDTH, WATERFALL_HEIGHT);
        self.error = None;
    }

    fn draw_source_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Source", |ui| {
            if ui.button("Open SigMF…").clicked() {
                ui.close();
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("SigMF metadata", &["sigmf-meta"])
                    .pick_file()
                {
                    self.open_sigmf(&path);
                }
            }
            ui.separator();
            let devices = SeifySource::enumerate();
            if devices.is_empty() {
                ui.label("no devices found");
            }
            for device in devices {
                if ui.button(&device.label).clicked() {
                    ui.close();
                    match self.sources.open_device(&device) {
                        Ok(id) => self.attach_view_to(id),
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
            }
            ui.separator();
            for id in self.sources.ids() {
                let selected = id == self.view.source_id;
                if ui
                    .selectable_label(selected, self.sources.name(id))
                    .clicked()
                {
                    ui.close();
                    self.attach_view_to(id);
                }
            }
            if self.sources.get(self.view.source_id).is_some()
                && ui.button("Close current source").clicked()
            {
                ui.close();
                self.worker = None;
                self.sources.close(self.view.source_id);
                self.view.source_id = 0;
            }
        });
    }

    fn drain_rows(&mut self) {
        if let Some(worker) = &self.worker {
            while let Some(frame) = worker.rows().try_recv() {
                self.waterfall.push_row(
                    &frame.row.power_db,
                    self.view.ref_level_db,
                    self.view.range_db,
                    &self.colormap,
                );
                self.ring.push(frame);
            }
        }
    }

    fn fps(&mut self) -> f32 {
        let now = Instant::now();
        self.frame_times.push(now);
        self.frame_times
            .retain(|instant| now.duration_since(*instant).as_secs_f32() < 1.0);
        self.frame_times.len() as f32
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_rows();
        let fps = self.fps();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                self.draw_source_menu(ui);
            });
        });

        if let Some(SourceHandle::Iq(source)) = self.sources.get_mut(self.view.source_id) {
            egui::Panel::right("source_panel")
                .default_size(240.0)
                .show(ui, |ui| {
                    if let Some(error) = draw_source_panel(ui, source.as_mut()) {
                        self.error = Some(error);
                    }
                });
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                match self.sources.get(self.view.source_id) {
                    Some(SourceHandle::Iq(source)) => {
                        ui.label(format!("{} · {:?}", source.info().name, source.status()))
                    }
                    Some(SourceHandle::Spectrum(source)) => {
                        ui.label(format!("{} · {:?}", source.info().name, source.status()))
                    }
                    None => ui.label("no source"),
                };
                if let Some(worker) = &self.worker {
                    let stats = worker.stats();
                    ui.label(format!("rows/s {:.0}", stats.rows_per_second));
                    ui.label(format!("drops {}", stats.input_dropped_blocks));
                    if stats.overloaded {
                        ui.colored_label(egui::Color32::RED, "OVERLOAD");
                    }
                }
                ui.label(format!("{fps:.0} fps"));
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            });
        });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let total = ui.available_rect_before_wrap();
            match self.view.mode {
                ViewMode::Spectrum => {
                    draw_spectrum(ui, &mut self.view, self.ring.latest());
                }
                ViewMode::Waterfall => self.waterfall.draw(ui, total),
                ViewMode::Both => {
                    let split_y = total.top() + total.height() * 0.4;
                    let spectrum_rect =
                        egui::Rect::from_min_max(total.min, egui::pos2(total.right(), split_y));
                    let waterfall_rect =
                        egui::Rect::from_min_max(egui::pos2(total.left(), split_y), total.max);
                    ui.scope_builder(egui::UiBuilder::new().max_rect(spectrum_rect), |ui| {
                        draw_spectrum(ui, &mut self.view, self.ring.latest());
                    });
                    self.waterfall.draw(ui, waterfall_rect);
                }
            }
        });

        ui.ctx().request_repaint();
    }
}
