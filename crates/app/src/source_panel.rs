use spectra_core::{IqSource, ParamKind, ParamValue};

#[derive(Clone, Copy, Default, PartialEq)]
enum PanelTab {
    #[default]
    Controls,
    Metadata,
}

/// Renders a source's header, then its controls, plus a metadata tab when the source has any.
/// The panel knows nothing about the device.
pub fn draw_source_panel(ui: &mut egui::Ui, source: &mut dyn IqSource) -> Option<String> {
    ui.heading(source.info().name);
    ui.label(format!("{:?}", source.status()));

    let tab_id = egui::Id::new("source_panel_tab");
    let mut selected_tab: PanelTab = ui.data(|data| data.get_temp(tab_id).unwrap_or_default());
    if source.metadata().is_none() {
        selected_tab = PanelTab::Controls;
    } else {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut selected_tab, PanelTab::Controls, "Controls");
            ui.selectable_value(&mut selected_tab, PanelTab::Metadata, "Metadata");
        });
        ui.data_mut(|data| data.insert_temp(tab_id, selected_tab));
    }
    ui.separator();

    match (selected_tab, source.metadata()) {
        (PanelTab::Metadata, Some(metadata)) => {
            draw_metadata(ui, metadata);
            None
        }
        _ => draw_controls(ui, source),
    }
}

/// Monospace, unwrapped metadata text that scrolls both ways inside the panel.
fn draw_metadata(ui: &mut egui::Ui, metadata: &str) {
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(metadata).monospace()).extend());
        });
}

/// Renders every parameter a source describes and applies edits; returns the last set error.
fn draw_controls(ui: &mut egui::Ui, source: &mut dyn IqSource) -> Option<String> {
    let mut error = None;
    for spec in source.params() {
        let Some(current) = source.param(&spec.key) else {
            continue;
        };
        let changed = match (&spec.kind, current) {
            (
                ParamKind::Range {
                    min,
                    max,
                    step,
                    unit,
                },
                ParamValue::Number(mut value),
            ) => ui
                .horizontal(|ui| {
                    ui.label(&spec.label);
                    let mut drag = egui::DragValue::new(&mut value)
                        .range(*min..=*max)
                        .speed(*step)
                        .suffix(format!(" {unit}"));
                    if *unit == "Hz" {
                        drag = drag
                            .custom_formatter(|hz, _| format_hz(hz))
                            .custom_parser(parse_hz);
                    }
                    ui.add(drag).changed()
                })
                .inner
                .then_some(ParamValue::Number(value)),
            (ParamKind::Toggle, ParamValue::Toggle(mut value)) => ui
                .checkbox(&mut value, &spec.label)
                .changed()
                .then_some(ParamValue::Toggle(value)),
            (ParamKind::Choice(options), ParamValue::Choice(mut value)) => {
                let before = value.clone();
                egui::ComboBox::from_label(&spec.label)
                    .selected_text(&value)
                    .show_ui(ui, |ui| {
                        for option in options {
                            ui.selectable_value(&mut value, option.clone(), option);
                        }
                    });
                (value != before).then_some(ParamValue::Choice(value))
            }
            _ => None,
        };
        if let Some(new_value) = changed
            && let Err(set_error) = source.set_param(&spec.key, new_value)
        {
            error = Some(format!("{}: {set_error:#}", spec.label));
        }
    }
    error
}

/// Hz with SI prefix and three decimals, as the toolbar shows frequencies.
pub fn format_hz(hz: f64) -> String {
    let magnitude = hz.abs();
    if magnitude >= 1e9 {
        format!("{:.3} GHz", hz / 1e9)
    } else if magnitude >= 1e6 {
        format!("{:.3} MHz", hz / 1e6)
    } else if magnitude >= 1e3 {
        format!("{:.3} kHz", hz / 1e3)
    } else {
        format!("{hz:.0} Hz")
    }
}

/// Accepts "915", "915M", "915 MHz", "2.4G", "433.92e6".
pub fn parse_hz(text: &str) -> Option<f64> {
    let trimmed = text
        .trim()
        .trim_end_matches("Hz")
        .trim_end_matches("hz")
        .trim();
    let (number, multiplier) = match trimmed.chars().last()? {
        'G' | 'g' => (&trimmed[..trimmed.len() - 1], 1e9),
        'M' | 'm' => (&trimmed[..trimmed.len() - 1], 1e6),
        'K' | 'k' => (&trimmed[..trimmed.len() - 1], 1e3),
        _ => (trimmed, 1.0),
    };
    number
        .trim()
        .parse::<f64>()
        .ok()
        .map(|value| value * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hz_round_trips_through_si_text() {
        assert_eq!(format_hz(915_000_000.0), "915.000 MHz");
        assert_eq!(parse_hz("915.000 MHz"), Some(915e6));
        assert_eq!(parse_hz("2.4G"), Some(2.4e9));
        assert_eq!(parse_hz("433.92e6"), Some(433.92e6));
        assert_eq!(parse_hz("nonsense"), None);
    }
}
