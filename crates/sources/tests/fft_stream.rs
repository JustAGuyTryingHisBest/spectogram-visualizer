use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

use spectra_core::{PowerUnits, SourceStatus, SpectrumSource};
use spectra_sources::FftStreamSource;

fn header(units: u8, center_hz: f64, span_hz: f64, bin_count: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"SPEC");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.push(units);
    bytes.push(0);
    bytes.extend_from_slice(&center_hz.to_le_bytes());
    bytes.extend_from_slice(&span_hz.to_le_bytes());
    bytes.extend_from_slice(&bin_count.to_le_bytes());
    bytes
}

fn frame(timestamp_ns: u64, power: &[f32]) -> Vec<u8> {
    let mut bytes = timestamp_ns.to_le_bytes().to_vec();
    bytes.extend_from_slice(&(power.len() as u32).to_le_bytes());
    for value in power {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn wait_for_status(
    source: &FftStreamSource,
    predicate: impl Fn(&SourceStatus) -> bool,
) -> SourceStatus {
    for _ in 0..100 {
        let status = source.status();
        if predicate(&status) {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    source.status()
}

#[test]
fn receives_rows_from_producer() {
    let source = FftStreamSource::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let subscriber = source.subscribe();
    let mut producer = TcpStream::connect(source.local_addr()).unwrap();
    producer.write_all(&header(1, 100e6, 1e6, 512)).unwrap();
    for frame_index in 0..3u64 {
        producer
            .write_all(&frame(frame_index, &vec![-60.0 - frame_index as f32; 512]))
            .unwrap();
    }
    for frame_index in 0..3 {
        let row = subscriber.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(row.center_hz, 100e6);
        assert_eq!(row.span_hz, 1e6);
        assert_eq!(row.units, PowerUnits::Dbm);
        assert_eq!(row.power_db.len(), 512);
        assert_eq!(row.power_db[0], -60.0 - frame_index as f32);
    }
    assert_eq!(source.info().center_hz, 100e6);
    assert_eq!(source.status(), SourceStatus::Streaming);
}

#[test]
fn rejects_bad_magic_and_accepts_next_connection() {
    let source = FftStreamSource::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let subscriber = source.subscribe();
    let mut bad = TcpStream::connect(source.local_addr()).unwrap();
    bad.write_all(b"NOPE").unwrap();
    bad.write_all(&[0u8; 24]).unwrap();
    let status = wait_for_status(
        &source,
        |status| matches!(status, SourceStatus::Stopped(reason) if reason.contains("bad magic")),
    );
    assert!(
        matches!(&status, SourceStatus::Stopped(reason) if reason.contains("bad magic")),
        "{status:?}"
    );

    let mut good = TcpStream::connect(source.local_addr()).unwrap();
    good.write_all(&header(0, 50e6, 2e6, 16)).unwrap();
    good.write_all(&frame(7, &[-20.0; 16])).unwrap();
    let row = subscriber.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(row.units, PowerUnits::Dbfs);
    assert_eq!(row.power_db.len(), 16);
}

#[test]
fn frame_bin_count_mismatch_closes_connection() {
    let source = FftStreamSource::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let subscriber = source.subscribe();
    let mut producer = TcpStream::connect(source.local_addr()).unwrap();
    producer.write_all(&header(0, 50e6, 2e6, 16)).unwrap();
    producer.write_all(&frame(1, &[-40.0; 8])).unwrap();
    let status = wait_for_status(
        &source,
        |status| matches!(status, SourceStatus::Stopped(reason) if reason.contains("bin count")),
    );
    assert!(
        matches!(&status, SourceStatus::Stopped(reason) if reason.contains("bin count")),
        "{status:?}"
    );
    assert!(subscriber.try_recv().is_none());
}

#[test]
fn short_frame_closes_connection() {
    let source = FftStreamSource::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let subscriber = source.subscribe();
    let mut producer = TcpStream::connect(source.local_addr()).unwrap();
    producer.write_all(&header(0, 50e6, 2e6, 512)).unwrap();
    // Valid prefix (timestamp + matching bin count) followed by only 100 of 2048 payload bytes.
    producer
        .write_all(&frame(1, &[-40.0; 512])[..12 + 100])
        .unwrap();
    drop(producer);
    let status = wait_for_status(
        &source,
        |status| matches!(status, SourceStatus::Stopped(reason) if reason != "waiting for producer"),
    );
    assert!(
        matches!(&status, SourceStatus::Stopped(reason) if reason.contains("reading frame")),
        "{status:?}"
    );
    assert!(subscriber.try_recv().is_none());

    let mut next = TcpStream::connect(source.local_addr()).unwrap();
    next.write_all(&header(0, 50e6, 2e6, 8)).unwrap();
    next.write_all(&frame(1, &[-30.0; 8])).unwrap();
    assert!(subscriber.recv_timeout(Duration::from_secs(2)).is_some());
}
