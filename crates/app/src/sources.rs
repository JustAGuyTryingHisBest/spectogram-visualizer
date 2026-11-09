use std::collections::BTreeMap;
use std::path::Path;

use std::net::{Ipv4Addr, SocketAddr};

use spectra_core::{IqSource, SourceHandle, SourceStatus};
use spectra_sources::{DeviceSummary, FftStreamSource, SeifySource, SigmfSource};

use crate::layout::SavedSource;
use crate::view::SourceId;

const DEFAULT_CENTER_HZ: f64 = 915e6;
const DEFAULT_SAMPLE_RATE_HZ: f64 = 10e6;

/// Owns every open source; views refer to them by id. A source that failed to reopen stays
/// listed with its error so it can be retried and is not lost from the saved layout.
#[derive(Default)]
pub struct SourceManager {
    sources: BTreeMap<SourceId, (Result<SourceHandle, String>, SavedSource)>,
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
        let source = SeifySource::open(&summary.args, DEFAULT_CENTER_HZ, DEFAULT_SAMPLE_RATE_HZ)?;
        let info = source.info();
        Ok(self.insert(
            SourceHandle::Iq(Box::new(source)),
            SavedSource::Device {
                args: summary.args.to_string(),
                center_hz: info.center_hz,
                sample_rate_hz: info.sample_rate_hz,
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

    /// Reopens a saved source under its old id so saved views still point at it. On failure the
    /// entry is kept with the error and the call returns it.
    pub fn reopen(&mut self, id: SourceId, saved: &SavedSource) -> anyhow::Result<()> {
        let outcome = open_saved(saved);
        let result = match &outcome {
            Ok(_) => Ok(()),
            Err(error) => Err(anyhow::anyhow!("{error}")),
        };
        self.sources.insert(id, (outcome, saved.clone()));
        self.next_id = self.next_id.max(id);
        result
    }

    /// Tries every failed source again; returns the ids that came back.
    pub fn retry_failed(&mut self) -> Vec<SourceId> {
        let mut recovered = Vec::new();
        for id in self.failed_ids() {
            let saved = self.sources[&id].1.clone();
            if self.reopen(id, &saved).is_ok() {
                recovered.push(id);
            }
        }
        recovered
    }

    pub fn failed_ids(&self) -> Vec<SourceId> {
        self.sources
            .iter()
            .filter(|(_, (handle, _))| handle.is_err())
            .map(|(id, _)| *id)
            .collect()
    }

    fn insert(&mut self, handle: SourceHandle, saved: SavedSource) -> SourceId {
        self.next_id += 1;
        self.sources.insert(self.next_id, (Ok(handle), saved));
        self.next_id
    }

    pub fn close(&mut self, id: SourceId) {
        self.sources.remove(&id);
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceHandle> {
        self.sources
            .get(&id)
            .and_then(|(handle, _)| handle.as_ref().ok())
    }

    pub fn get_mut(&mut self, id: SourceId) -> Option<&mut SourceHandle> {
        self.sources
            .get_mut(&id)
            .and_then(|(handle, _)| handle.as_mut().ok())
    }

    pub fn ids(&self) -> Vec<SourceId> {
        self.sources.keys().copied().collect()
    }

    pub fn name(&self, id: SourceId) -> String {
        match self.sources.get(&id) {
            Some((Ok(SourceHandle::Iq(source)), _)) => source.info().name,
            Some((Ok(SourceHandle::Spectrum(source)), _)) => source.info().name,
            Some((Err(error), saved)) => format!("{} (failed: {error})", describe_saved(saved)),
            None => "closed".into(),
        }
    }

    /// Live status of a source, or the open error for one that failed to reopen.
    pub fn status(&self, id: SourceId) -> Option<SourceStatus> {
        match self.sources.get(&id)? {
            (Ok(SourceHandle::Iq(source)), _) => Some(source.status()),
            (Ok(SourceHandle::Spectrum(source)), _) => Some(source.status()),
            (Err(error), _) => Some(SourceStatus::Stopped(error.clone())),
        }
    }

    /// Everything needed to reopen the current sources, with devices' current tuning.
    pub fn saved(&self) -> Vec<(SourceId, SavedSource)> {
        self.sources
            .iter()
            .map(|(id, (handle, saved))| {
                let saved = match (handle, saved) {
                    (Ok(SourceHandle::Iq(source)), SavedSource::Device { args, .. }) => {
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

fn open_saved(saved: &SavedSource) -> Result<SourceHandle, String> {
    let handle = match saved {
        SavedSource::Sigmf { path } => SourceHandle::Iq(Box::new(
            SigmfSource::open(path).map_err(|error| format!("{error:#}"))?,
        )),
        SavedSource::Device {
            args,
            center_hz,
            sample_rate_hz,
        } => {
            let args = seify::Args::from(args.as_str()).map_err(|error| error.to_string())?;
            SourceHandle::Iq(Box::new(
                SeifySource::open(&args, *center_hz, *sample_rate_hz)
                    .map_err(|error| format!("{error:#}"))?,
            ))
        }
        SavedSource::FftStream { port } => SourceHandle::Spectrum(Box::new(
            FftStreamSource::listen(SocketAddr::from((Ipv4Addr::LOCALHOST, *port)))
                .map_err(|error| format!("{error:#}"))?,
        )),
    };
    Ok(handle)
}

fn describe_saved(saved: &SavedSource) -> String {
    match saved {
        SavedSource::Sigmf { path } => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "sigmf".into()),
        SavedSource::Device { args, .. } => args.clone(),
        SavedSource::FftStream { port } => format!("fft-stream :{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn failed_source_is_kept_for_retry() {
        let mut manager = SourceManager::default();
        let saved = SavedSource::Sigmf {
            path: PathBuf::from("/nonexistent/recording.sigmf-meta"),
        };
        assert!(manager.reopen(7, &saved).is_err());
        assert!(manager.get(7).is_none());
        assert!(manager.ids().contains(&7));
        assert!(manager.name(7).contains("failed"));
        assert_eq!(manager.saved(), vec![(7, saved)]);
        assert_eq!(manager.failed_ids(), vec![7]);
    }
}
