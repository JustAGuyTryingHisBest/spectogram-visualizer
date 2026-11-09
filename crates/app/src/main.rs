mod app;
mod colormap;
mod source_panel;
mod sources;
mod spectrum_plot;
mod view;
mod view_toolbar;
mod waterfall;

use std::path::PathBuf;

fn main() -> eframe::Result {
    let sigmf_path: Option<PathBuf> = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("spectra")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "spectra",
        options,
        Box::new(move |_creation_context| Ok(Box::new(app::App::new(sigmf_path.as_deref())))),
    )
}
