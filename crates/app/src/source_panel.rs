use spectra_core::{IqSource, ParamKind, ParamValue};

/// Renders every parameter a source describes; the panel knows nothing about the device.
pub fn draw_source_panel(ui: &mut egui::Ui, source: &mut dyn IqSource) -> Option<String> {
    let mut error = None;
    ui.heading(source.info().name);
    ui.label(format!("{:?}", source.status()));
    ui.separator();
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
