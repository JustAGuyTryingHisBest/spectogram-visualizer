use crate::params::{ParamSpec, ParamValue};
use crate::types::{IqBlock, SpectrumRow, StreamInfo};

/// Placeholder until the broadcast fan-out lands; wraps a plain receiver.
pub struct Subscriber<T>(pub crossbeam_channel::Receiver<T>);

#[derive(Clone, Debug, PartialEq)]
pub enum SourceStatus {
    Streaming,
    Paused,
    Stopped(String),
}

/// A device or file that yields complex baseband samples.
pub trait IqSource: Send {
    fn info(&self) -> StreamInfo;
    fn subscribe(&self) -> Subscriber<IqBlock>;
    fn params(&self) -> Vec<ParamSpec>;
    fn param(&self, key: &str) -> Option<ParamValue>;
    fn set_param(&mut self, key: &str, value: ParamValue) -> anyhow::Result<()>;
    fn status(&self) -> SourceStatus;
}

/// Something that already delivers finished spectrum rows.
pub trait SpectrumSource: Send {
    fn info(&self) -> StreamInfo;
    fn subscribe(&self) -> Subscriber<SpectrumRow>;
    fn status(&self) -> SourceStatus;
}

pub enum SourceHandle {
    Iq(Box<dyn IqSource>),
    Spectrum(Box<dyn SpectrumSource>),
}
