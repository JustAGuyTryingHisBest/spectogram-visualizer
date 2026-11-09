use std::path::{Path, PathBuf};

use anyhow::Context;
use egui_dock::DockState;
use serde::{Deserialize, Serialize};

use crate::view::{SourceId, View, ViewId};

/// How to reopen a source on the next launch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SavedSource {
    Sigmf {
        path: PathBuf,
    },
    Device {
        args: String,
        center_hz: f64,
        sample_rate_hz: f64,
    },
    FftStream {
        port: u16,
    },
}

#[derive(Serialize, Deserialize)]
pub struct SavedLayout {
    pub dock: DockState<ViewId>,
    pub views: Vec<View>,
    pub sources: Vec<(SourceId, SavedSource)>,
}

pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("spectra").join("layout.ron"))
}

pub fn save(path: &Path, layout: &SavedLayout) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = ron::ser::to_string_pretty(layout, ron::ser::PrettyConfig::default())?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

pub fn load(path: &Path) -> anyhow::Result<SavedLayout> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    ron::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::View;
    use std::path::PathBuf;

    #[test]
    fn layout_round_trips_through_ron() {
        let dir = std::env::temp_dir().join(format!("spectra-layout-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("layout.ron");
        let mut view = View::new(3, 1);
        view.config.fft_size = 4096;
        view.markers.add_at(915e6);
        let layout = SavedLayout {
            dock: DockState::new(vec![3]),
            views: vec![view],
            sources: vec![
                (
                    1,
                    SavedSource::Sigmf {
                        path: PathBuf::from("/tmp/test.sigmf-meta"),
                    },
                ),
                (
                    2,
                    SavedSource::Device {
                        args: "driver=hackrf".into(),
                        center_hz: 915e6,
                        sample_rate_hz: 20e6,
                    },
                ),
                (3, SavedSource::FftStream { port: 4590 }),
            ],
        };
        save(&path, &layout).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.views[0].config.fft_size, 4096);
        assert_eq!(loaded.views[0].markers.get(1).unwrap().hz, 915e6);
        assert_eq!(loaded.sources, layout.sources);
        assert_eq!(loaded.dock.iter_all_tabs().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
