/// A device-described tunable, rendered generically by the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    Range {
        min: f64,
        max: f64,
        step: f64,
        unit: &'static str,
    },
    Toggle,
    Choice(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    pub kind: ParamKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    Number(f64),
    Toggle(bool),
    Choice(String),
}

/// Keys every IQ source honors when its hardware allows.
pub const PARAM_CENTER_HZ: &str = "center_hz";
pub const PARAM_SAMPLE_RATE_HZ: &str = "sample_rate_hz";
pub const PARAM_GAIN_DB: &str = "gain_db";
