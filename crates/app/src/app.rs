use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use egui_dock::widgets::tab_viewer::OnCloseResponse;
use egui_dock::{DockArea, DockState, NodePath, TabViewer};
use spectra_core::SourceHandle;
use spectra_dsp::ChainWorker;
use spectra_sources::SeifySource;

use crate::colormap::Colormap;
use crate::layout::{self, SavedLayout};
use crate::source_panel::draw_source_panel;
use crate::sources::SourceManager;
use crate::spectrum_plot::draw_spectrum;
use crate::view::{SourceId, View, ViewId, ViewMode, ViewRuntime};
use crate::view_toolbar::draw_view_toolbar;

pub struct App {
    sources: SourceManager,
    dock: DockState<ViewId>,
    views: BTreeMap<ViewId, ViewRuntime>,
    next_view_id: ViewId,
    selected_source: Option<SourceId>,
    fft_stream_port: u16,
    colormap: Colormap,
    frame_times: Vec<Instant>,
    error: Option<String>,
}

impl App {
    pub fn new(sigmf_path: Option<&Path>) -> Self {
        let mut app = Self {
            sources: SourceManager::default(),
            dock: DockState::new(Vec::new()),
            views: BTreeMap::new(),
            next_view_id: 0,
            selected_source: None,
            fft_stream_port: 4590,
            colormap: Colormap::inferno(),
            frame_times: Vec::new(),
            error: None,
        };
        if let Some(path) = sigmf_path {
            app.open_sigmf(path);
        } else if let Some(layout_path) = layout::default_path()
            && layout_path.exists()
        {
            match layout::load(&layout_path) {
                Ok(saved) => app.restore(saved),
                Err(error) => app.error = Some(format!("{error:#}")),
            }
        }
        app
    }

    /// Rebuilds sources and views from a saved layout; sources that fail to reopen leave
    /// their views in place showing the error rather than aborting the restore.
    fn restore(&mut self, saved: SavedLayout) {
        let mut failures = Vec::new();
        for (id, source) in &saved.sources {
            if let Err(error) = self.sources.reopen(*id, source) {
                failures.push(format!("source {id}: {error:#}"));
            }
        }
        for view in saved.views {
            let view_id = view.id;
            let mut runtime = ViewRuntime::new(view);
            match self.sources.get(runtime.view.source_id) {
                Some(SourceHandle::Iq(source)) => {
                    runtime.worker = Some(ChainWorker::spawn(
                        source.subscribe(),
                        runtime.view.config.clone(),
                        256,
                    ));
                }
                Some(SourceHandle::Spectrum(source)) => {
                    runtime.spectrum_input = Some(source.subscribe());
                }
                None => {}
            }
            self.next_view_id = self.next_view_id.max(view_id);
            self.views.insert(view_id, runtime);
        }
        self.dock = saved.dock;
        self.selected_source = self.sources.ids().first().copied();
        if !failures.is_empty() {
            self.error = Some(failures.join("; "));
        }
    }

    fn save_layout(&self) {
        let Some(path) = layout::default_path() else {
            return;
        };
        let layout = SavedLayout {
            dock: self.dock.clone(),
            views: self
                .views
                .values()
                .map(|runtime| runtime.view.clone())
                .collect(),
            sources: self.sources.saved(),
        };
        if let Err(error) = layout::save(&path, &layout) {
            eprintln!("saving layout: {error:#}");
        }
    }

    fn open_sigmf(&mut self, path: &Path) {
        match self.sources.open_sigmf(path) {
            Ok(id) => self.on_source_opened(id),
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    fn on_source_opened(&mut self, source_id: SourceId) {
        self.selected_source = Some(source_id);
        self.error = None;
        self.add_view(source_id);
    }

    /// Creates a view on `source_id` with its own chain worker and docks it as a new tab.
    fn add_view(&mut self, source_id: SourceId) {
        self.next_view_id += 1;
        let view_id = self.next_view_id;
        let mut runtime = ViewRuntime::new(View::new(view_id, source_id));
        match self.sources.get(source_id) {
            Some(SourceHandle::Iq(source)) => {
                runtime.worker = Some(ChainWorker::spawn(
                    source.subscribe(),
                    runtime.view.config.clone(),
                    256,
                ));
            }
            Some(SourceHandle::Spectrum(source)) => {
                runtime.spectrum_input = Some(source.subscribe());
            }
            None => {}
        }
        self.views.insert(view_id, runtime);
        self.dock.push_to_focused_leaf(view_id);
    }

    fn close_source(&mut self, source_id: SourceId) {
        let doomed: Vec<ViewId> = self
            .views
            .iter()
            .filter(|(_, runtime)| runtime.view.source_id == source_id)
            .map(|(id, _)| *id)
            .collect();
        for view_id in doomed {
            if let Some(path) = self.dock.find_tab(&view_id) {
                self.dock.remove_tab(path);
            }
            self.views.remove(&view_id);
        }
        self.sources.close(source_id);
        if self.selected_source == Some(source_id) {
            self.selected_source = self.sources.ids().first().copied();
        }
    }

    fn draw_menus(&mut self, ui: &mut egui::Ui) {
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
            ui.horizontal(|ui| {
                if ui.button("Listen for FFT stream on port").clicked() {
                    ui.close();
                    match self.sources.listen_fft_stream(self.fft_stream_port) {
                        Ok(id) => self.on_source_opened(id),
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
                ui.add(egui::DragValue::new(&mut self.fft_stream_port).range(1..=65535));
            });
            ui.separator();
            let devices = SeifySource::enumerate();
            if devices.is_empty() {
                ui.label("no devices found");
            }
            for device in devices {
                if ui.button(&device.label).clicked() {
                    ui.close();
                    match self.sources.open_device(&device) {
                        Ok(id) => self.on_source_opened(id),
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
            }
            ui.separator();
            for id in self.sources.ids() {
                let selected = self.selected_source == Some(id);
                if ui
                    .selectable_label(selected, self.sources.name(id))
                    .clicked()
                {
                    ui.close();
                    self.selected_source = Some(id);
                }
            }
            if let Some(selected) = self.selected_source
                && ui.button("Close selected source").clicked()
            {
                ui.close();
                self.close_source(selected);
            }
        });
        ui.menu_button("View", |ui| {
            if self.sources.ids().is_empty() {
                ui.label("open a source first");
            }
            for id in self.sources.ids() {
                if ui
                    .button(format!("New view on {}", self.sources.name(id)))
                    .clicked()
                {
                    ui.close();
                    self.add_view(id);
                }
            }
        });
    }

    /// Median of the newest row of the first view: a cheap, robust noise-floor estimate.
    fn noise_floor_db(&self) -> Option<f32> {
        let frame = self
            .views
            .values()
            .find_map(|runtime| runtime.ring.latest())?;
        let mut sorted: Vec<f32> = frame.row.power_db.to_vec();
        sorted.sort_by(f32::total_cmp);
        sorted.get(sorted.len() / 2).copied()
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
    fn on_exit(&mut self) {
        self.save_layout();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for runtime in self.views.values_mut() {
            runtime.drain_worker(&self.colormap);
        }
        let fps = self.fps();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| self.draw_menus(ui));
        });

        if let Some(SourceHandle::Iq(source)) =
            self.selected_source.and_then(|id| self.sources.get_mut(id))
        {
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
                let mut total_drops = 0;
                let mut overloaded = false;
                let mut rows_per_second = 0.0;
                for runtime in self.views.values() {
                    if let Some(worker) = &runtime.worker {
                        let stats = worker.stats();
                        total_drops += stats.input_dropped_blocks;
                        overloaded |= stats.overloaded;
                        rows_per_second += stats.rows_per_second;
                    }
                }
                ui.label(format!("{} views", self.views.len()));
                ui.label(format!("rows/s {rows_per_second:.0}"));
                if let Some(floor_db) = self.noise_floor_db() {
                    ui.label(format!("floor {floor_db:.0} dB"));
                }
                ui.label(format!("drops {total_drops}"));
                if overloaded {
                    ui.colored_label(egui::Color32::RED, "OVERLOAD");
                }
                ui.label(format!("{fps:.0} fps"));
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            });
        });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            let mut tabs = Tabs {
                views: &mut self.views,
                sources: &self.sources,
            };
            DockArea::new(&mut self.dock)
                .show_add_buttons(false)
                .show_inside(ui, &mut tabs);
        });

        ui.ctx().request_repaint();
    }
}

struct Tabs<'a> {
    views: &'a mut BTreeMap<ViewId, ViewRuntime>,
    sources: &'a SourceManager,
}

impl TabViewer for Tabs<'_> {
    type Tab = ViewId;

    fn id(&mut self, tab: &mut ViewId) -> egui::Id {
        egui::Id::new(("view", *tab))
    }

    fn title(&mut self, tab: &mut ViewId) -> egui::WidgetText {
        match self.views.get(tab) {
            Some(runtime) => {
                format!("{} · {}", tab, self.sources.name(runtime.view.source_id)).into()
            }
            None => "closed".into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut ViewId) {
        let Some(runtime) = self.views.get_mut(tab) else {
            ui.label("view closed");
            return;
        };
        let source_name = self.sources.name(runtime.view.source_id);
        if draw_view_toolbar(ui, runtime, &source_name) {
            runtime.apply_config();
        }
        let total = ui.available_rect_before_wrap();
        match runtime.view.mode {
            ViewMode::Spectrum => {
                draw_spectrum(ui, &mut runtime.view, runtime.ring.latest());
            }
            ViewMode::Waterfall => runtime.waterfall.draw(ui, total),
            ViewMode::Both => {
                let split_y = total.top() + total.height() * 0.4;
                let spectrum_rect =
                    egui::Rect::from_min_max(total.min, egui::pos2(total.right(), split_y));
                let waterfall_rect =
                    egui::Rect::from_min_max(egui::pos2(total.left(), split_y), total.max);
                ui.scope_builder(egui::UiBuilder::new().max_rect(spectrum_rect), |ui| {
                    draw_spectrum(ui, &mut runtime.view, runtime.ring.latest());
                });
                runtime.waterfall.draw(ui, waterfall_rect);
            }
        }
    }

    fn on_close(&mut self, tab: &mut ViewId) -> OnCloseResponse {
        self.views.remove(tab);
        OnCloseResponse::Close
    }

    fn context_menu(&mut self, _ui: &mut egui::Ui, _tab: &mut ViewId, _path: NodePath) {}

    fn clear_background(&self, _tab: &ViewId) -> bool {
        true
    }
}
