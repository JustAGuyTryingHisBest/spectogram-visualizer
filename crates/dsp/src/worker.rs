use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, unbounded};
use spectra_core::{Broadcast, IqBlock, PowerUnits, SpectrumRow, Subscriber};

use crate::chain::{Chain, ChainConfig};

/// One finished row plus the traces the chain derived from it.
#[derive(Clone)]
pub struct RowFrame {
    pub row: SpectrumRow,
    pub average_db: Arc<[f32]>,
    pub max_hold_db: Option<Arc<[f32]>>,
    pub source_dropped_blocks: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WorkerStats {
    pub rows_per_second: f32,
    pub input_dropped_blocks: u64,
    /// True when the input subscriber lost blocks within the last second.
    pub overloaded: bool,
}

enum Control {
    SetConfig(ChainConfig),
    ResetMaxHold,
}

struct SharedStats {
    rows_per_second_bits: AtomicU64,
    input_dropped_blocks: AtomicU64,
    last_drop_at: Mutex<Option<Instant>>,
    stop: AtomicBool,
    stopped: AtomicBool,
}

/// Runs a `Chain` on its own thread, fed by a source subscriber, emitting `RowFrame`s.
pub struct ChainWorker {
    rows: Subscriber<RowFrame>,
    control: Sender<Control>,
    stats: Arc<SharedStats>,
}

impl ChainWorker {
    pub fn spawn(input: Subscriber<IqBlock>, config: ChainConfig, row_capacity: usize) -> Self {
        let rows_broadcast = Arc::new(Broadcast::new());
        let rows = rows_broadcast.subscribe(row_capacity);
        let (control_sender, control_receiver) = unbounded();
        let stats = Arc::new(SharedStats {
            rows_per_second_bits: AtomicU64::new(0),
            input_dropped_blocks: AtomicU64::new(0),
            last_drop_at: Mutex::new(None),
            stop: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        let thread_stats = Arc::clone(&stats);
        thread::Builder::new()
            .name("chain-worker".into())
            .spawn(move || {
                run(
                    input,
                    config,
                    control_receiver,
                    rows_broadcast,
                    thread_stats,
                );
            })
            .expect("spawning chain worker thread");
        Self {
            rows,
            control: control_sender,
            stats,
        }
    }

    pub fn rows(&self) -> &Subscriber<RowFrame> {
        &self.rows
    }

    pub fn set_config(&self, config: ChainConfig) {
        let _ = self.control.send(Control::SetConfig(config));
    }

    pub fn reset_max_hold(&self) {
        let _ = self.control.send(Control::ResetMaxHold);
    }

    pub fn stats(&self) -> WorkerStats {
        let last_drop_at = *self.stats.last_drop_at.lock().unwrap();
        WorkerStats {
            rows_per_second: f32::from_bits(
                self.stats.rows_per_second_bits.load(Ordering::Relaxed) as u32,
            ),
            input_dropped_blocks: self.stats.input_dropped_blocks.load(Ordering::Relaxed),
            overloaded: last_drop_at
                .is_some_and(|instant| instant.elapsed() < Duration::from_secs(1)),
        }
    }

    /// True once the input source is gone and the thread has exited.
    pub fn is_stopped(&self) -> bool {
        self.stats.stopped.load(Ordering::Relaxed)
    }
}

impl Drop for ChainWorker {
    fn drop(&mut self) {
        self.stats.stop.store(true, Ordering::Relaxed);
    }
}

fn run(
    input: Subscriber<IqBlock>,
    initial_config: ChainConfig,
    control: crossbeam_channel::Receiver<Control>,
    rows: Arc<Broadcast<RowFrame>>,
    stats: Arc<SharedStats>,
) {
    let mut chain = Chain::new(initial_config);
    let mut ddc: Option<crate::subband::Ddc> = None;
    let mut ddc_input_rate = 0.0;
    let mut decimated = Vec::new();
    let mut rows_in_window = 0u32;
    let mut window_started = Instant::now();
    let mut last_seen_dropped = 0u64;

    loop {
        while let Ok(message) = control.try_recv() {
            match message {
                Control::SetConfig(config) => {
                    chain = Chain::new(config);
                    ddc = None;
                }
                Control::ResetMaxHold => chain.reset_max_hold(),
            }
        }
        if stats.stop.load(Ordering::Relaxed) {
            break;
        }
        let Some(block) = input.recv_timeout(Duration::from_millis(50)) else {
            if input.is_closed() {
                break;
            }
            continue;
        };

        let dropped_now = input.dropped();
        if dropped_now != last_seen_dropped {
            last_seen_dropped = dropped_now;
            stats
                .input_dropped_blocks
                .store(dropped_now, Ordering::Relaxed);
            *stats.last_drop_at.lock().unwrap() = Some(Instant::now());
        }

        let (center_hz, span_hz) = match chain.config().sub_band {
            Some(sub_band) => {
                if ddc.is_none() || ddc_input_rate != block.info.sample_rate_hz {
                    ddc = Some(crate::subband::Ddc::new(
                        sub_band,
                        block.info.sample_rate_hz,
                    ));
                    ddc_input_rate = block.info.sample_rate_hz;
                }
                (
                    block.info.center_hz + sub_band.offset_hz,
                    block.info.sample_rate_hz / f64::from(sub_band.decimation),
                )
            }
            None => (block.info.center_hz, block.info.sample_rate_hz),
        };
        let samples: &[num_complex::Complex32] = match ddc.as_mut() {
            Some(ddc) => {
                decimated.clear();
                ddc.process(&block.samples, &mut decimated);
                &decimated
            }
            None => &block.samples,
        };

        let (units, offset_db) = match block.info.calibration_offset_db {
            Some(offset) => (PowerUnits::Dbm, offset),
            None => (PowerUnits::Dbfs, 0.0),
        };
        let received_at = block.received_at;
        let dropped_for_frame = dropped_now;
        chain.push(samples, &mut |output| {
            let power_db: Arc<[f32]> = if offset_db == 0.0 {
                output.live_db
            } else {
                output.live_db.iter().map(|db| db + offset_db).collect()
            };
            rows.publish(RowFrame {
                row: SpectrumRow {
                    center_hz,
                    span_hz,
                    units,
                    power_db,
                    received_at,
                },
                average_db: output.average_db,
                max_hold_db: output.max_hold_db,
                source_dropped_blocks: dropped_for_frame,
            });
            rows_in_window += 1;
        });

        let elapsed = window_started.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let rate = rows_in_window as f32 / elapsed.as_secs_f32();
            stats
                .rows_per_second_bits
                .store(u64::from(rate.to_bits()), Ordering::Relaxed);
            rows_in_window = 0;
            window_started = Instant::now();
        }
    }
    stats.stopped.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_complex::Complex32;
    use spectra_core::{Broadcast, IqBlock, PowerUnits, StreamInfo};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn info(calibration_offset_db: Option<f32>) -> StreamInfo {
        StreamInfo {
            name: "test".into(),
            center_hz: 100e6,
            sample_rate_hz: 2e6,
            calibration_offset_db,
        }
    }

    fn block(info: &StreamInfo, sequence: u64, sample_count: usize) -> IqBlock {
        IqBlock {
            info: info.clone(),
            samples: Arc::from(vec![Complex32::new(0.5, 0.0); sample_count]),
            sequence,
            received_at: Instant::now(),
        }
    }

    #[test]
    fn worker_emits_rows_with_source_geometry() {
        let broadcast = Broadcast::new();
        let worker = ChainWorker::spawn(
            broadcast.subscribe(16),
            ChainConfig {
                fft_size: 1024,
                ..ChainConfig::default()
            },
            64,
        );
        let info = info(None);
        broadcast.publish(block(&info, 0, 4096));
        let frame = worker.rows().recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(frame.row.center_hz, 100e6);
        assert_eq!(frame.row.span_hz, 2e6);
        assert_eq!(frame.row.power_db.len(), 1024);
        assert_eq!(frame.row.units, PowerUnits::Dbfs);
        assert_eq!(frame.average_db.len(), 1024);
    }

    #[test]
    fn calibration_offset_switches_units_to_dbm() {
        let broadcast = Broadcast::new();
        let worker = ChainWorker::spawn(
            broadcast.subscribe(16),
            ChainConfig {
                fft_size: 256,
                window: crate::Window::Rectangular,
                ..ChainConfig::default()
            },
            64,
        );
        let info = info(Some(-30.0));
        broadcast.publish(block(&info, 0, 256));
        let frame = worker.rows().recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(frame.row.units, PowerUnits::Dbm);
        // A DC tone of amplitude 0.5 is -6.02 dBFS; with the offset it reads about -36 dBm.
        let dc_bin = frame.row.power_db[128];
        assert!((dc_bin + 36.02).abs() < 0.1, "{dc_bin}");
    }

    #[test]
    fn overload_is_reported_when_input_drops() {
        let broadcast = Broadcast::new();
        let worker = ChainWorker::spawn(
            broadcast.subscribe(1),
            ChainConfig {
                fft_size: 65536,
                ..ChainConfig::default()
            },
            4,
        );
        let info = info(None);
        for sequence in 0..200 {
            broadcast.publish(block(&info, sequence, 65536));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && worker.stats().input_dropped_blocks == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        let stats = worker.stats();
        assert!(stats.input_dropped_blocks > 0);
        assert!(stats.overloaded);
    }

    #[test]
    fn worker_stops_when_source_is_dropped() {
        let broadcast = Broadcast::<IqBlock>::new();
        let worker = ChainWorker::spawn(broadcast.subscribe(16), ChainConfig::default(), 64);
        drop(broadcast);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && !worker.is_stopped() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(worker.is_stopped());
    }

    #[test]
    fn sub_band_sets_row_geometry() {
        let broadcast = Broadcast::new();
        let worker = ChainWorker::spawn(
            broadcast.subscribe(16),
            ChainConfig {
                fft_size: 256,
                sub_band: Some(crate::SubBand {
                    offset_hz: 250e3,
                    decimation: 4,
                }),
                ..ChainConfig::default()
            },
            64,
        );
        let info = info(None);
        broadcast.publish(block(&info, 0, 4096));
        let frame = worker.rows().recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(frame.row.center_hz, 100.25e6);
        assert_eq!(frame.row.span_hz, 500e3);
        assert_eq!(frame.row.power_db.len(), 256);
    }

    #[test]
    fn set_config_rebuilds_chain() {
        let broadcast = Broadcast::new();
        let worker = ChainWorker::spawn(
            broadcast.subscribe(16),
            ChainConfig {
                fft_size: 1024,
                ..ChainConfig::default()
            },
            64,
        );
        worker.set_config(ChainConfig {
            fft_size: 512,
            ..ChainConfig::default()
        });
        std::thread::sleep(Duration::from_millis(20));
        let info = info(None);
        broadcast.publish(block(&info, 0, 512));
        let frame = worker.rows().recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(frame.row.power_db.len(), 512);
    }
}
