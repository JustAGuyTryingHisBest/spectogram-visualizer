use spectra_core::SourceStatus;
use spectra_dsp::{SubBand, Window};

use crate::colormap::ColormapKind;
use crate::source_panel::{format_hz, parse_hz};
use crate::view::{ViewMode, ViewRuntime};

const FFT_SIZES: [usize; 9] = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536];
const DECIMATIONS: [u32; 6] = [2, 4, 8, 16, 32, 64];

/// Per-view controls. Returns true when the chain configuration changed.
pub fn draw_view_toolbar(
    ui: &mut egui::Ui,
    runtime: &mut ViewRuntime,
    source_name: &str,
    source_status: Option<&SourceStatus>,
) -> bool {
    let view = &mut runtime.view;
    let mut config_changed = false;
    let mut reset_requested = false;
    let mut colormap_changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.strong(source_name);
        match source_status {
            Some(SourceStatus::Streaming) => {
                ui.colored_label(egui::Color32::from_rgb(0x6f, 0xd4, 0x9a), "● streaming");
            }
            Some(SourceStatus::Paused) => {
                ui.colored_label(egui::Color32::YELLOW, "❚❚ paused");
            }
            Some(SourceStatus::Stopped(reason)) => {
                ui.colored_label(egui::Color32::RED, format!("■ stopped: {reason}"));
            }
            None => {
                ui.colored_label(egui::Color32::RED, "■ source closed");
            }
        }
        ui.separator();

        egui::ComboBox::from_id_salt(("fft", view.id))
            .selected_text(format!("FFT {}", view.config.fft_size))
            .show_ui(ui, |ui| {
                for size in FFT_SIZES {
                    config_changed |= ui
                        .selectable_value(&mut view.config.fft_size, size, size.to_string())
                        .changed();
                }
            });
        egui::ComboBox::from_id_salt(("window", view.id))
            .selected_text(view.config.window.label())
            .show_ui(ui, |ui| {
                for window in Window::ALL {
                    config_changed |= ui
                        .selectable_value(&mut view.config.window, window, window.label())
                        .changed();
                }
            });
        ui.label("avg");
        config_changed |= ui
            .add(egui::DragValue::new(&mut view.config.averaging).range(1..=256))
            .changed();
        config_changed |= ui.checkbox(&mut view.config.max_hold, "max hold").changed();
        if ui.button("reset").clicked() {
            reset_requested = true;
        }
        ui.separator();

        let mut decimation = view.config.sub_band.map(|sub_band| sub_band.decimation);
        egui::ComboBox::from_id_salt(("decimation", view.id))
            .selected_text(match decimation {
                Some(value) => format!("sub-band ÷{value}"),
                None => "full span".into(),
            })
            .show_ui(ui, |ui| {
                config_changed |= ui
                    .selectable_value(&mut decimation, None, "full span")
                    .changed();
                for value in DECIMATIONS {
                    config_changed |= ui
                        .selectable_value(&mut decimation, Some(value), format!("÷{value}"))
                        .changed();
                }
            });
        match decimation {
            Some(decimation) => {
                let sub_band = view.config.sub_band.get_or_insert(SubBand {
                    offset_hz: 0.0,
                    decimation,
                });
                sub_band.decimation = decimation;
                ui.label("offset");
                config_changed |= ui
                    .add(
                        egui::DragValue::new(&mut sub_band.offset_hz)
                            .speed(1000.0)
                            .custom_formatter(|hz, _| format_hz(hz))
                            .custom_parser(parse_hz),
                    )
                    .changed();
            }
            None => view.config.sub_band = None,
        }
        ui.separator();

        ui.label("ref");
        ui.add(
            egui::DragValue::new(&mut view.ref_level_db)
                .speed(1.0)
                .range(-150.0..=50.0)
                .suffix(" dB"),
        );
        ui.label("range");
        ui.add(
            egui::DragValue::new(&mut view.range_db)
                .speed(1.0)
                .range(10.0..=200.0)
                .suffix(" dB"),
        );
        ui.separator();

        egui::ComboBox::from_id_salt(("colormap", view.id))
            .selected_text(view.colormap.label())
            .show_ui(ui, |ui| {
                for kind in ColormapKind::ALL {
                    colormap_changed |= ui
                        .selectable_value(&mut view.colormap, kind, kind.label())
                        .changed();
                }
            });
        ui.separator();

        ui.checkbox(&mut view.show_live, "live");
        ui.checkbox(&mut view.show_average, "avg");
        ui.checkbox(&mut view.show_max_hold, "max");
        ui.separator();

        for (mode, label) in [
            (ViewMode::Spectrum, "spectrum"),
            (ViewMode::Waterfall, "waterfall"),
            (ViewMode::Both, "both"),
        ] {
            ui.selectable_value(&mut view.mode, mode, label);
        }
    });
    if reset_requested {
        runtime.reset_max_hold();
    }
    if colormap_changed {
        runtime.apply_colormap();
    }
    config_changed
}
