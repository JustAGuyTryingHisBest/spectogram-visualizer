use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use rustfft::num_complex::Complex32;
use serde::Deserialize;
use spectra_core::{
    Broadcast, IqBlock, IqSource, ParamKind, ParamSpec, ParamValue, SourceStatus, StreamInfo,
    Subscriber,
};

const BLOCK_SAMPLES: usize = 16384;

#[derive(Deserialize)]
struct Metadata {
    global: Global,
    #[serde(default)]
    captures: Vec<Capture>,
}

#[derive(Deserialize)]
struct Global {
    #[serde(rename = "core:datatype")]
    datatype: String,
    #[serde(rename = "core:sample_rate")]
    sample_rate: Option<f64>,
}

#[derive(Deserialize)]
struct Capture {
    #[serde(rename = "core:frequency")]
    frequency: Option<f64>,
}

#[derive(Clone, Copy)]
enum SampleFormat {
    Cf32Le,
    Ci16Le,
    Ci8,
    Cu8,
}

impl SampleFormat {
    fn parse(datatype: &str) -> anyhow::Result<Self> {
        match datatype {
            "cf32_le" => Ok(Self::Cf32Le),
            "ci16_le" => Ok(Self::Ci16Le),
            "ci8" => Ok(Self::Ci8),
            "cu8" => Ok(Self::Cu8),
            other => bail!("unsupported datatype {other:?}"),
        }
    }

    fn bytes_per_sample(self) -> u64 {
        match self {
            Self::Cf32Le => 8,
            Self::Ci16Le => 4,
            Self::Ci8 | Self::Cu8 => 2,
        }
    }

    fn convert(self, bytes: &[u8], out: &mut Vec<Complex32>) {
        match self {
            Self::Cf32Le => {
                for chunk in bytes.as_chunks::<8>().0 {
                    let (re, im) = chunk.split_at(4);
                    out.push(Complex32::new(
                        f32::from_le_bytes(re.try_into().unwrap()),
                        f32::from_le_bytes(im.try_into().unwrap()),
                    ));
                }
            }
            Self::Ci16Le => {
                for chunk in bytes.as_chunks::<4>().0 {
                    let (re, im) = chunk.split_at(2);
                    out.push(Complex32::new(
                        i16::from_le_bytes(re.try_into().unwrap()) as f32 / 32768.0,
                        i16::from_le_bytes(im.try_into().unwrap()) as f32 / 32768.0,
                    ));
                }
            }
            Self::Ci8 => {
                for chunk in bytes.as_chunks::<2>().0 {
                    out.push(Complex32::new(
                        chunk[0] as i8 as f32 / 128.0,
                        chunk[1] as i8 as f32 / 128.0,
                    ));
                }
            }
            Self::Cu8 => {
                for chunk in bytes.as_chunks::<2>().0 {
                    out.push(Complex32::new(
                        (chunk[0] as f32 - 127.5) / 127.5,
                        (chunk[1] as f32 - 127.5) / 127.5,
                    ));
                }
            }
        }
    }
}

struct Shared {
    position: AtomicU64,
    paused: AtomicBool,
    looping: AtomicBool,
    stop: AtomicBool,
    speed: Mutex<f64>,
    status: Mutex<SourceStatus>,
}

/// Replays a SigMF recording as if it were a live radio.
pub struct SigmfSource {
    info: StreamInfo,
    total_samples: u64,
    shared: Arc<Shared>,
    broadcast: Arc<Broadcast<IqBlock>>,
}

impl SigmfSource {
    /// `path` is the `.sigmf-meta` file; the `.sigmf-data` sibling is required.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let meta_text =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let metadata: Metadata =
            serde_json::from_str(&meta_text).context("parsing SigMF metadata")?;
        let format = SampleFormat::parse(&metadata.global.datatype)?;
        let sample_rate_hz = metadata
            .global
            .sample_rate
            .ok_or_else(|| anyhow!("core:sample_rate missing"))?;
        if !(sample_rate_hz > 0.0 && sample_rate_hz.is_finite()) {
            bail!("core:sample_rate must be a positive number, got {sample_rate_hz}");
        }
        let center_hz = metadata
            .captures
            .first()
            .and_then(|capture| capture.frequency)
            .unwrap_or(0.0);

        let data_path: PathBuf = path.with_extension("sigmf-data");
        let data_len = fs::metadata(&data_path)
            .with_context(|| format!("reading {}", data_path.display()))?
            .len();
        if data_len % format.bytes_per_sample() != 0 {
            bail!(
                "{} is not a whole number of samples ({data_len} bytes, {} per sample)",
                data_path.display(),
                format.bytes_per_sample()
            );
        }
        let total_samples = data_len / format.bytes_per_sample();

        let info = StreamInfo {
            name: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "sigmf".into()),
            center_hz,
            sample_rate_hz,
            calibration_offset_db: None,
        };
        let shared = Arc::new(Shared {
            position: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            speed: Mutex::new(1.0),
            status: Mutex::new(SourceStatus::Streaming),
        });
        let broadcast = Arc::new(Broadcast::new());

        let reader = Reader {
            file: fs::File::open(&data_path)?,
            format,
            total_samples,
            info: info.clone(),
            shared: Arc::clone(&shared),
            broadcast: Arc::clone(&broadcast),
        };
        thread::Builder::new()
            .name("sigmf-reader".into())
            .spawn(move || reader.run())?;

        Ok(Self {
            info,
            total_samples,
            shared,
            broadcast,
        })
    }

    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    pub fn position(&self) -> u64 {
        self.shared.position.load(Ordering::Relaxed)
    }

    pub fn seek(&self, sample_index: u64) {
        self.shared
            .position
            .store(sample_index.min(self.total_samples), Ordering::Relaxed);
        *self.shared.status.lock().unwrap() = if self.shared.paused.load(Ordering::Relaxed) {
            SourceStatus::Paused
        } else {
            SourceStatus::Streaming
        };
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
        let mut status = self.shared.status.lock().unwrap();
        if !matches!(*status, SourceStatus::Stopped(_)) {
            *status = if paused {
                SourceStatus::Paused
            } else {
                SourceStatus::Streaming
            };
        }
    }

    pub fn set_loop(&self, looping: bool) {
        self.shared.looping.store(looping, Ordering::Relaxed);
    }

    pub fn set_speed(&self, multiplier: f64) {
        *self.shared.speed.lock().unwrap() = multiplier.clamp(0.1, 8.0);
    }
}

impl Drop for SigmfSource {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl IqSource for SigmfSource {
    fn info(&self) -> StreamInfo {
        self.info.clone()
    }

    fn subscribe(&self) -> Subscriber<IqBlock> {
        self.broadcast.subscribe(32)
    }

    fn params(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec {
                key: "paused".into(),
                label: "Paused".into(),
                kind: ParamKind::Toggle,
            },
            ParamSpec {
                key: "loop".into(),
                label: "Loop".into(),
                kind: ParamKind::Toggle,
            },
            ParamSpec {
                key: "speed".into(),
                label: "Speed".into(),
                kind: ParamKind::Range {
                    min: 0.1,
                    max: 8.0,
                    step: 0.1,
                    unit: "x",
                },
            },
            ParamSpec {
                key: "position".into(),
                label: "Position".into(),
                kind: ParamKind::Range {
                    min: 0.0,
                    max: self.total_samples as f64,
                    step: (self.total_samples / 1000).max(1) as f64,
                    unit: "samples",
                },
            },
        ]
    }

    fn param(&self, key: &str) -> Option<ParamValue> {
        match key {
            "paused" => Some(ParamValue::Toggle(
                self.shared.paused.load(Ordering::Relaxed),
            )),
            "loop" => Some(ParamValue::Toggle(
                self.shared.looping.load(Ordering::Relaxed),
            )),
            "speed" => Some(ParamValue::Number(*self.shared.speed.lock().unwrap())),
            "position" => Some(ParamValue::Number(self.position() as f64)),
            _ => None,
        }
    }

    fn set_param(&mut self, key: &str, value: ParamValue) -> anyhow::Result<()> {
        match (key, value) {
            ("paused", ParamValue::Toggle(paused)) => self.set_paused(paused),
            ("loop", ParamValue::Toggle(looping)) => self.set_loop(looping),
            ("speed", ParamValue::Number(speed)) => self.set_speed(speed),
            ("position", ParamValue::Number(sample_index)) => {
                self.seek(sample_index.max(0.0) as u64)
            }
            (key, value) => bail!("unknown parameter {key} = {value:?}"),
        }
        Ok(())
    }

    fn status(&self) -> SourceStatus {
        self.shared.status.lock().unwrap().clone()
    }
}

struct Reader {
    file: fs::File,
    format: SampleFormat,
    total_samples: u64,
    info: StreamInfo,
    shared: Arc<Shared>,
    broadcast: Arc<Broadcast<IqBlock>>,
}

impl Reader {
    fn run(mut self) {
        let mut byte_buffer = vec![0u8; BLOCK_SAMPLES * self.format.bytes_per_sample() as usize];
        let mut samples = Vec::with_capacity(BLOCK_SAMPLES);
        let mut sequence = 0u64;
        let mut next_deadline = Instant::now();

        while !self.shared.stop.load(Ordering::Relaxed) {
            if self.shared.paused.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(10));
                next_deadline = Instant::now();
                continue;
            }
            let position = self.shared.position.load(Ordering::Relaxed);
            if position >= self.total_samples {
                if self.shared.looping.load(Ordering::Relaxed) {
                    self.shared.position.store(0, Ordering::Relaxed);
                    continue;
                }
                *self.shared.status.lock().unwrap() = SourceStatus::Stopped("end of file".into());
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            let block_samples = BLOCK_SAMPLES.min((self.total_samples - position) as usize);
            let byte_count = block_samples * self.format.bytes_per_sample() as usize;
            if let Err(error) = self
                .file
                .seek(SeekFrom::Start(position * self.format.bytes_per_sample()))
                .and_then(|_| self.file.read_exact(&mut byte_buffer[..byte_count]))
            {
                *self.shared.status.lock().unwrap() = SourceStatus::Stopped(error.to_string());
                return;
            }
            samples.clear();
            self.format
                .convert(&byte_buffer[..byte_count], &mut samples);
            // A seek that landed while we were reading must win over our advance.
            let _ = self.shared.position.compare_exchange(
                position,
                position + block_samples as u64,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            *self.shared.status.lock().unwrap() = SourceStatus::Streaming;

            let speed = *self.shared.speed.lock().unwrap();
            let block_duration =
                Duration::from_secs_f64(block_samples as f64 / (self.info.sample_rate_hz * speed));
            next_deadline += block_duration;
            let now = Instant::now();
            if next_deadline > now {
                thread::sleep(next_deadline - now);
            } else {
                next_deadline = now;
            }

            self.broadcast.publish(IqBlock {
                info: self.info.clone(),
                samples: Arc::from(samples.as_slice()),
                sequence,
                received_at: Instant::now(),
            });
            sequence += 1;
        }
    }
}
