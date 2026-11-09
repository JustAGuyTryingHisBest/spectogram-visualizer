use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, bail};
use rustfft::num_complex::Complex32;
use seify::{Args, DynDevice, RxStreamer};
use spectra_core::{
    Broadcast, IqBlock, IqSource, ParamKind, ParamSpec, ParamValue, SourceStatus, StreamInfo,
    Subscriber,
};

const BLOCK_SAMPLES: usize = 16384;
const READ_TIMEOUT_US: i64 = 200_000;
const RESTART_ACK_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// One device found by enumeration, ready to open.
#[derive(Clone, Debug)]
pub struct DeviceSummary {
    pub label: String,
    pub args: Args,
}

struct Shared {
    info: Mutex<StreamInfo>,
    status: Mutex<SourceStatus>,
    stop: AtomicBool,
    /// Set while the UI changes the sample rate so the reader pauses the stream.
    restart: AtomicBool,
    /// Raised by the reader once it has deactivated the stream in response to `restart`.
    reader_idle: AtomicBool,
    /// Whether the last rate change saw the reader go idle before the rate was set.
    restart_acknowledged: AtomicBool,
    overruns: AtomicU64,
}

/// Any radio seify can open: HackRF, RTL-SDR, USRP B2xx.
pub struct SeifySource {
    device: DynDevice,
    shared: Arc<Shared>,
    broadcast: Arc<Broadcast<IqBlock>>,
}

impl SeifySource {
    pub fn enumerate() -> Vec<DeviceSummary> {
        seify::enumerate()
            .unwrap_or_default()
            .into_iter()
            .map(|args| DeviceSummary {
                label: describe(&args),
                args,
            })
            .collect()
    }

    pub fn open(
        args: &Args,
        initial_center_hz: f64,
        initial_sample_rate_hz: f64,
    ) -> anyhow::Result<Self> {
        let device = DynDevice::from_args(args.clone()).context("opening device")?;
        let rx = device.rx(0).context("device has no RX channel")?;
        // Devices differ widely in supported rates (RTL-SDR tops out near 3.2 MS/s).
        let initial_sample_rate_hz = rx
            .sample_rate()
            .range()
            .map(|range| range.closest(initial_sample_rate_hz))
            .unwrap_or(initial_sample_rate_hz);
        rx.sample_rate()
            .set(initial_sample_rate_hz)
            .context("setting sample rate")?;
        rx.frequency()
            .set(initial_center_hz)
            .context("setting center frequency")?;
        let info = StreamInfo {
            name: describe(args),
            center_hz: rx.frequency().value().unwrap_or(initial_center_hz),
            sample_rate_hz: rx.sample_rate().value().unwrap_or(initial_sample_rate_hz),
            calibration_offset_db: None,
        };
        let shared = Arc::new(Shared {
            info: Mutex::new(info),
            status: Mutex::new(SourceStatus::Streaming),
            stop: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            reader_idle: AtomicBool::new(false),
            restart_acknowledged: AtomicBool::new(false),
            overruns: AtomicU64::new(0),
        });
        let broadcast = Arc::new(Broadcast::new());

        let reader_device = device.clone();
        let reader_shared = Arc::clone(&shared);
        let reader_broadcast = Arc::clone(&broadcast);
        thread::Builder::new()
            .name("sdr-reader".into())
            .spawn(move || read_loop(reader_device, reader_shared, reader_broadcast))?;

        Ok(Self {
            device,
            shared,
            broadcast,
        })
    }

    /// Blocks dropped by the device itself (USB overruns), as opposed to subscriber drops.
    pub fn overruns(&self) -> u64 {
        self.shared.overruns.load(Ordering::Relaxed)
    }

    /// True when the last sample-rate change waited for the reader to stop streaming first.
    pub fn reader_acknowledged_restart(&self) -> bool {
        self.shared.restart_acknowledged.load(Ordering::SeqCst)
    }

    fn rx(&self) -> anyhow::Result<seify::RxChannel<'_, DynDevice>> {
        self.device.rx(0).context("RX channel 0")
    }

    fn range_param(
        &self,
        key: &str,
        label: &str,
        unit: &'static str,
        range: Option<seify::Range>,
    ) -> Option<ParamSpec> {
        let range = range?;
        let step = match unit {
            "Hz" => 1.0,
            _ => 0.5,
        };
        Some(ParamSpec {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Range {
                min: range.min(),
                max: range.max().min(1e12),
                step,
                unit,
            },
        })
    }
}

impl Drop for SeifySource {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl IqSource for SeifySource {
    fn info(&self) -> StreamInfo {
        self.shared.info.lock().unwrap().clone()
    }

    fn subscribe(&self) -> Subscriber<IqBlock> {
        self.broadcast.subscribe(32)
    }

    fn params(&self) -> Vec<ParamSpec> {
        let Ok(rx) = self.rx() else {
            return Vec::new();
        };
        let mut specs = Vec::new();
        specs.extend(self.range_param("center_hz", "Center", "Hz", rx.frequency().range().ok()));
        specs.extend(self.range_param(
            "sample_rate_hz",
            "Sample rate",
            "Hz",
            rx.sample_rate().range().ok(),
        ));
        specs.extend(self.range_param("gain_db", "Gain", "dB", rx.gain().range().ok()));
        for element in rx.gain().elements().unwrap_or_default() {
            specs.extend(self.range_param(
                &format!("gain:{element}"),
                &element,
                "dB",
                rx.gain().element(&element).range().ok(),
            ));
        }
        if let Ok(ports) = rx.antenna().ports()
            && ports.len() > 1
        {
            specs.push(ParamSpec {
                key: "antenna".into(),
                label: "Antenna".into(),
                kind: ParamKind::Choice(ports),
            });
        }
        if rx.agc().enabled().is_ok() {
            specs.push(ParamSpec {
                key: "agc".into(),
                label: "AGC".into(),
                kind: ParamKind::Toggle,
            });
        }
        specs
    }

    fn param(&self, key: &str) -> Option<ParamValue> {
        let rx = self.rx().ok()?;
        match key {
            "center_hz" => Some(ParamValue::Number(self.info().center_hz)),
            "sample_rate_hz" => Some(ParamValue::Number(self.info().sample_rate_hz)),
            "gain_db" => rx.gain().value().ok().flatten().map(ParamValue::Number),
            "antenna" => rx.antenna().selected().ok().map(ParamValue::Choice),
            "agc" => rx.agc().enabled().ok().map(ParamValue::Toggle),
            other => other.strip_prefix("gain:").and_then(|element| {
                rx.gain()
                    .element(element)
                    .value()
                    .ok()
                    .flatten()
                    .map(ParamValue::Number)
            }),
        }
    }

    fn set_param(&mut self, key: &str, value: ParamValue) -> anyhow::Result<()> {
        let rx = self.rx()?;
        match (key, &value) {
            ("center_hz", ParamValue::Number(hz)) => {
                rx.frequency().set(*hz)?;
                self.shared.info.lock().unwrap().center_hz = rx.frequency().value().unwrap_or(*hz);
            }
            ("sample_rate_hz", ParamValue::Number(rate)) => {
                // The reader deactivates its stream while this flag is set; wait for it so the
                // rate never changes under a live stream.
                self.shared.restart.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + RESTART_ACK_TIMEOUT;
                while !self.shared.reader_idle.load(Ordering::SeqCst) && Instant::now() < deadline {
                    thread::sleep(std::time::Duration::from_millis(1));
                }
                self.shared.restart_acknowledged.store(
                    self.shared.reader_idle.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                );
                let result = rx.sample_rate().set(*rate);
                self.shared.info.lock().unwrap().sample_rate_hz =
                    rx.sample_rate().value().unwrap_or(*rate);
                self.shared.restart.store(false, Ordering::SeqCst);
                result?;
            }
            ("gain_db", ParamValue::Number(gain)) => rx.gain().set(*gain)?,
            ("antenna", ParamValue::Choice(port)) => rx.antenna().select(port)?,
            ("agc", ParamValue::Toggle(enabled)) => rx.agc().set_enabled(*enabled)?,
            (other, ParamValue::Number(gain)) if other.starts_with("gain:") => {
                rx.gain().element(&other["gain:".len()..]).set(*gain)?;
            }
            (key, value) => bail!("unknown parameter {key} = {value:?}"),
        }
        Ok(())
    }

    fn status(&self) -> SourceStatus {
        self.shared.status.lock().unwrap().clone()
    }
}

fn describe(args: &Args) -> String {
    let driver = args
        .get::<String>("driver")
        .unwrap_or_else(|_| "sdr".into());
    let detail = ["label", "serial", "id"]
        .iter()
        .find_map(|key| args.get::<String>(key).ok());
    match detail {
        Some(detail) => format!("{driver} {detail}"),
        None => driver,
    }
}

/// Reads blocks from the device and hands them to subscribers; nothing else.
fn read_loop(device: DynDevice, shared: Arc<Shared>, broadcast: Arc<Broadcast<IqBlock>>) {
    let mut streamer = match device.rx_streamer(&[0]) {
        Ok(streamer) => streamer,
        Err(error) => {
            *shared.status.lock().unwrap() = SourceStatus::Stopped(error.to_string());
            return;
        }
    };
    if let Err(error) = streamer.activate() {
        *shared.status.lock().unwrap() = SourceStatus::Stopped(error.to_string());
        return;
    }
    let mut buffer = vec![Complex32::default(); BLOCK_SAMPLES];
    let mut sequence = 0u64;
    let mut active = true;
    while !shared.stop.load(Ordering::Relaxed) {
        if shared.restart.load(Ordering::SeqCst) {
            if active {
                let _ = streamer.deactivate();
                active = false;
            }
            shared.reader_idle.store(true, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        if !active {
            if let Err(error) = streamer.activate() {
                *shared.status.lock().unwrap() = SourceStatus::Stopped(error.to_string());
                return;
            }
            active = true;
            shared.reader_idle.store(false, Ordering::SeqCst);
        }
        match streamer.read(&mut [&mut buffer], READ_TIMEOUT_US) {
            Ok(0) => continue,
            Ok(count) => {
                let info = shared.info.lock().unwrap().clone();
                broadcast.publish(IqBlock {
                    info,
                    samples: Arc::from(&buffer[..count]),
                    sequence,
                    received_at: Instant::now(),
                });
                sequence += 1;
            }
            // Timeouts and device overruns are routine; only other errors end the stream.
            Err(seify::Error::Timeout) => continue,
            Err(seify::Error::Io(error)) if error.kind() == std::io::ErrorKind::TimedOut => {
                continue;
            }
            Err(seify::Error::Overrun) => {
                shared.overruns.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            Err(error) => {
                *shared.status.lock().unwrap() = SourceStatus::Stopped(error.to_string());
                return;
            }
        }
    }
    let _ = streamer.deactivate();
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectra_core::{IqSource, ParamValue};
    use std::time::Duration;

    #[test]
    fn dummy_device_exposes_core_params() {
        let args = seify::Args::from("driver=dummy").unwrap();
        let mut source = SeifySource::open(&args, 915e6, 2e6).unwrap();
        let keys: Vec<String> = source.params().into_iter().map(|spec| spec.key).collect();
        for expected in ["center_hz", "sample_rate_hz", "gain_db"] {
            assert!(keys.contains(&expected.to_string()), "{keys:?}");
        }
        source
            .set_param("center_hz", ParamValue::Number(433e6))
            .unwrap();
        assert_eq!(source.info().center_hz, 433e6);
        assert_eq!(source.param("center_hz"), Some(ParamValue::Number(433e6)));
    }

    #[test]
    fn sample_rate_change_pauses_the_reader_then_resumes() {
        let args = seify::Args::from("driver=dummy").unwrap();
        let mut source = SeifySource::open(&args, 915e6, 2e6).unwrap();
        let subscriber = source.subscribe();
        assert!(subscriber.recv_timeout(Duration::from_secs(1)).is_some());
        source
            .set_param("sample_rate_hz", ParamValue::Number(4e6))
            .unwrap();
        assert!(source.reader_acknowledged_restart());
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut saw_new_rate = false;
        while Instant::now() < deadline {
            if let Some(block) = subscriber.try_recv() {
                saw_new_rate |= block.info.sample_rate_hz == 4e6;
            }
        }
        assert!(saw_new_rate);
    }
}
