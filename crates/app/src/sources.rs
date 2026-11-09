use std::collections::BTreeMap;
use std::path::Path;

use std::net::{Ipv4Addr, SocketAddr};

use spectra_core::SourceHandle;
use spectra_sources::{DeviceSummary, FftStreamSource, SeifySource, SigmfSource};

use crate::view::SourceId;

const DEFAULT_CENTER_HZ: f64 = 915e6;
const DEFAULT_SAMPLE_RATE_HZ: f64 = 10e6;

/// Owns every open source; views refer to them by id.
#[derive(Default)]
pub struct SourceManager {
    sources: BTreeMap<SourceId, SourceHandle>,
    next_id: SourceId,
}

impl SourceManager {
    pub fn open_sigmf(&mut self, path: &Path) -> anyhow::Result<SourceId> {
        let source = SigmfSource::open(path)?;
        Ok(self.insert(SourceHandle::Iq(Box::new(source))))
    }

    pub fn open_device(&mut self, summary: &DeviceSummary) -> anyhow::Result<SourceId> {
        let source = SeifySource::open(&summary.args, DEFAULT_CENTER_HZ, DEFAULT_SAMPLE_RATE_HZ)?;
        Ok(self.insert(SourceHandle::Iq(Box::new(source))))
    }

    pub fn listen_fft_stream(&mut self, port: u16) -> anyhow::Result<SourceId> {
        let source = FftStreamSource::listen(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        Ok(self.insert(SourceHandle::Spectrum(Box::new(source))))
    }

    pub fn insert(&mut self, handle: SourceHandle) -> SourceId {
        self.next_id += 1;
        self.sources.insert(self.next_id, handle);
        self.next_id
    }

    pub fn close(&mut self, id: SourceId) {
        self.sources.remove(&id);
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceHandle> {
        self.sources.get(&id)
    }

    pub fn get_mut(&mut self, id: SourceId) -> Option<&mut SourceHandle> {
        self.sources.get_mut(&id)
    }

    pub fn ids(&self) -> Vec<SourceId> {
        self.sources.keys().copied().collect()
    }

    pub fn name(&self, id: SourceId) -> String {
        match self.sources.get(&id) {
            Some(SourceHandle::Iq(source)) => source.info().name,
            Some(SourceHandle::Spectrum(source)) => source.info().name,
            None => "closed".into(),
        }
    }
}
