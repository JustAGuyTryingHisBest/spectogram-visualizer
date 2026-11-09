//! Lists the radios seify can see, then streams one second from the first and reports the rate.
use std::time::{Duration, Instant};

use spectra_core::IqSource;
use spectra_sources::SeifySource;

fn main() -> anyhow::Result<()> {
    let devices = SeifySource::enumerate();
    if devices.is_empty() {
        println!("no devices found");
        return Ok(());
    }
    for device in &devices {
        println!("found: {} ({})", device.label, device.args);
    }
    let source = SeifySource::open(&devices[0].args, 915e6, 10e6)?;
    println!("opened: {:?}", source.info());
    for spec in source.params() {
        println!("  param {:?} = {:?}", spec, source.param(&spec.key));
    }
    let subscriber = source.subscribe();
    let started = Instant::now();
    let mut samples = 0usize;
    while started.elapsed() < Duration::from_secs(1) {
        if let Some(block) = subscriber.recv_timeout(Duration::from_millis(200)) {
            samples += block.samples.len();
        }
    }
    println!(
        "{samples} samples in {:?} ({:.2} MS/s), dropped {}, status {:?}",
        started.elapsed(),
        samples as f64 / started.elapsed().as_secs_f64() / 1e6,
        subscriber.dropped(),
        source.status()
    );
    Ok(())
}
