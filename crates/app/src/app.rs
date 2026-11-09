use std::path::Path;
use std::time::Instant;

use spectra_core::IqSource;
use spectra_dsp::ChainWorker;
use spectra_sources::SigmfSource;

use crate::spectrum_plot::draw_spectrum;
use crate::view::{RowRing, View};

const ROW_RING_CAPACITY: usize = 4096;

pub struct App {
    source: Option<Box<dyn IqSource>>,
    worker: Option<ChainWorker>,
    view: View,
    ring: RowRing,
    frame_times: Vec<Instant>,
    error: Option<String>,
}

impl App {
    pub fn new(sigmf_path: Option<&Path>) -> Self {
        let mut app = Self {
            source: None,
            worker: None,
            view: View::new(1, 1),
            ring: RowRing::new(ROW_RING_CAPACITY),
            frame_times: Vec::new(),
            error: None,
        };
        if let Some(path) = sigmf_path {
            match SigmfSource::open(path) {
                Ok(source) => app.attach_source(Box::new(source)),
                Err(error) => app.error = Some(format!("{error:#}")),
            }
        }
        app
    }

    fn attach_source(&mut self, source: Box<dyn IqSource>) {
        let worker = ChainWorker::spawn(source.subscribe(), self.view.config.clone(), 256);
        self.worker = Some(worker);
        self.source = Some(source);
    }

    fn drain_rows(&mut self) {
        if let Some(worker) = &self.worker {
            while let Some(frame) = worker.rows().try_recv() {
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

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                match &self.source {
                    Some(source) => {
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
            draw_spectrum(ui, &mut self.view, self.ring.latest());
        });

        ui.ctx().request_repaint();
    }
}
