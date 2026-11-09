use std::collections::BTreeMap;
use std::path::Path;

use std::net::{Ipv4Addr, SocketAddr};

use spectra_core::SourceHandle;
use spectra_sources::{DeviceSummary, FftStreamSource, SeifySource, SigmfSource};

use crate::layout::SavedSource;
use crate::view::SourceId;

const DEFAULT_CENTER_HZ: f64 = 915e6;
const DEFAULT_SAMPLE_RATE_HZ: f64 = 10e6;

/// Owns every open source; views refer to them by id.
#[derive(Default)]
pub struct SourceManager {
    sources: BTreeMap<SourceId, (SourceHandle, SavedSource)>,
    next_id: SourceId,
}

impl SourceManager {
    pub fn open_sigmf(&mut self, path: &Path) -> anyhow::Result<SourceId> {
        let source = SigmfSource::open(path)?;
        Ok(self.insert(
            SourceHandle::Iq(Box::new(source)),
            SavedSource::Sigmf {
                path: path.to_path_buf(),
            },
        ))
    }

    pub fn open_device(&mut self, summary: &DeviceSummary) -> anyhow::Result<SourceId> {
        self.open_device_at(&summary.args, DEFAULT_CENTER_HZ, DEFAULT_SAMPLE_RATE_HZ)
    }

    pub fn open_device_at(
        &mut self,
        args: &seify::Args,
        center_hz: f64,
        sample_rate_hz: f64,
    ) -> anyhow::Result<SourceId> {
        let source = SeifySource::open(args, center_hz, sample_rate_hz)?;
        Ok(self.insert(
            SourceHandle::Iq(Box::new(source)),
            SavedSource::Device {
                args: args.to_string(),
                center_hz,
                sample_rate_hz,
            },
        ))
    }

    pub fn listen_fft_stream(&mut self, port: u16) -> anyhow::Result<SourceId> {
        let source = FftStreamSource::listen(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        Ok(self.insert(
            SourceHandle::Spectrum(Box::new(source)),
            SavedSource::FftStream { port },
        ))
    }

    /// Reopens a saved source under its old id so saved views still point at it.
    pub fn reopen(&mut self, id: SourceId, saved: &SavedSource) -> anyhow::Result<()> {
        let handle = match saved {
            SavedSource::Sigmf { path } => SourceHandle::Iq(Box::new(SigmfSource::open(path)?)),
            SavedSource::Device {
                args,
                center_hz,
                sample_rate_hz,
            } => {
                let args = seify::Args::from(args.as_str())?;
                SourceHandle::Iq(Box::new(SeifySource::open(
                    &args,
                    *center_hz,
                    *sample_rate_hz,
                )?))
            }
            SavedSource::FftStream { port } => SourceHandle::Spectrum(Box::new(
                FftStreamSource::listen(SocketAddr::from((Ipv4Addr::LOCALHOST, *port)))?,
            )),
        };
        self.sources.insert(id, (handle, saved.clone()));
        self.next_id = self.next_id.max(id);
        Ok(())
    }

    fn insert(&mut self, handle: SourceHandle, saved: SavedSource) -> SourceId {
        self.next_id += 1;
        self.sources.insert(self.next_id, (handle, saved));
        self.next_id
    }

    pub fn close(&mut self, id: SourceId) {
        self.sources.remove(&id);
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceHandle> {
        self.sources.get(&id).map(|(handle, _)| handle)
    }

    pub fn get_mut(&mut self, id: SourceId) -> Option<&mut SourceHandle> {
        self.sources.get_mut(&id).map(|(handle, _)| handle)
    }

    pub fn ids(&self) -> Vec<SourceId> {
        self.sources.keys().copied().collect()
    }

    pub fn name(&self, id: SourceId) -> String {
        match self.get(id) {
            Some(SourceHandle::Iq(source)) => source.info().name,
            Some(SourceHandle::Spectrum(source)) => source.info().name,
            None => "closed".into(),
        }
    }

    /// Everything needed to reopen the current sources, with devices' current tuning.
    pub fn saved(&self) -> Vec<(SourceId, SavedSource)> {
        self.sources
            .iter()
            .map(|(id, (handle, saved))| {
                let saved = match (handle, saved) {
                    (SourceHandle::Iq(source), SavedSource::Device { args, .. }) => {
                        let info = source.info();
                        SavedSource::Device {
                            args: args.clone(),
                            center_hz: info.center_hz,
                            sample_rate_hz: info.sample_rate_hz,
                        }
                    }
                    (_, other) => other.clone(),
                };
                (*id, saved)
            })
            .collect()
    }
}
