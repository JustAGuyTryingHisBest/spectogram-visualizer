use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use spectra_core::{IqSource, ParamValue, SourceStatus};
use spectra_sources::SigmfSource;

#[test]
fn rejects_non_positive_sample_rate() {
    let dir = tempfile::tempdir().unwrap();
    let meta = write_recording(dir.path(), "cf32_le", &cf32_bytes(16), 0.0);
    let error = SigmfSource::open(&meta).err().unwrap().to_string();
    assert!(error.contains("sample_rate"), "{error}");
}

#[test]
fn position_param_seeks_while_paused() {
    let dir = tempfile::tempdir().unwrap();
    let meta = write_recording(dir.path(), "cf32_le", &cf32_bytes(65536), 1e6);
    let mut source = SigmfSource::open(&meta).unwrap();
    source
        .set_param("paused", ParamValue::Toggle(true))
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    source
        .set_param("position", ParamValue::Number(40000.0))
        .unwrap();
    assert_eq!(source.param("position"), Some(ParamValue::Number(40000.0)));
    assert!(source.params().iter().any(|spec| spec.key == "position"));
}

fn write_recording(dir: &Path, datatype: &str, data: &[u8], sample_rate: f64) -> PathBuf {
    let meta_path = dir.join("rec.sigmf-meta");
    let meta = format!(
        r#"{{"global":{{"core:datatype":"{datatype}","core:sample_rate":{sample_rate},"core:version":"1.0.0"}},
            "captures":[{{"core:sample_start":0,"core:frequency":915000000.0}}],"annotations":[]}}"#
    );
    fs::write(&meta_path, meta).unwrap();
    fs::write(dir.join("rec.sigmf-data"), data).unwrap();
    meta_path
}

fn cf32_bytes(sample_count: usize) -> Vec<u8> {
    (0..sample_count * 2)
        .flat_map(|index| (index as f32 * 0.001).to_le_bytes())
        .collect()
}

#[test]
fn plays_cf32_at_requested_rate() {
    let dir = tempfile::tempdir().unwrap();
    let meta = write_recording(dir.path(), "cf32_le", &cf32_bytes(65536), 1e6);
    let mut source = SigmfSource::open(&meta).unwrap();
    assert_eq!(source.info().center_hz, 915e6);
    assert_eq!(source.info().sample_rate_hz, 1e6);
    assert_eq!(source.total_samples(), 65536);
    let subscriber = source.subscribe();
    source.set_param("speed", ParamValue::Number(8.0)).unwrap();

    let started = Instant::now();
    let mut received = 0usize;
    let mut blocks = 0usize;
    while let Some(block) = subscriber.recv_timeout(Duration::from_secs(2)) {
        received += block.samples.len();
        blocks += 1;
        if blocks == 4 {
            assert!(
                started.elapsed() < Duration::from_millis(150),
                "{:?}",
                started.elapsed()
            );
        }
        if received == 65536 {
            break;
        }
    }
    assert_eq!(received, 65536);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(source.status(), SourceStatus::Stopped("end of file".into()));
}

#[test]
fn rejects_truncated_data() {
    let dir = tempfile::tempdir().unwrap();
    let meta = write_recording(dir.path(), "cf32_le", &[0u8; 7], 1e6);
    let error = SigmfSource::open(&meta).err().unwrap().to_string();
    assert!(error.contains("not a whole number of samples"), "{error}");
}

#[test]
fn rejects_unknown_datatype() {
    let dir = tempfile::tempdir().unwrap();
    let meta = write_recording(dir.path(), "cf64_le", &[0u8; 16], 1e6);
    let error = SigmfSource::open(&meta).err().unwrap().to_string();
    assert!(error.contains("unsupported datatype"), "{error}");
}

#[test]
fn loop_wraps_and_seek_moves() {
    let dir = tempfile::tempdir().unwrap();
    let meta = write_recording(dir.path(), "ci16_le", &vec![0u8; 4096 * 4], 1e6);
    let source = SigmfSource::open(&meta).unwrap();
    let subscriber = source.subscribe();
    source.set_loop(true);
    source.set_speed(8.0);
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut received = 0usize;
    while received <= 4096 && Instant::now() < deadline {
        if let Some(block) = subscriber.recv_timeout(Duration::from_millis(200)) {
            received += block.samples.len();
        }
    }
    assert!(received > 4096, "received {received}");
    source.set_paused(true);
    std::thread::sleep(Duration::from_millis(20));
    source.seek(0);
    assert!(source.position() < 4096);
}
